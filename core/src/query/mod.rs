use std::cmp::Ordering;
use std::collections::{BTreeSet, HashMap};
use std::str::FromStr;
use std::time::Instant;

use nom::branch::alt;
use nom::bytes::complete::{escaped_transform, is_not, tag, take_while1};
use nom::character::complete::{char, multispace0};
use nom::combinator::map;
use nom::multi::many0;
use nom::sequence::{delimited, preceded};

use crate::clock::{LamportTimestamp, ReplicaId};
use crate::model::{IssueDetails, MilestoneDetails};
use crate::repo::CacheGenerationSnapshot;

#[derive(Debug, thiserror::Error)]
pub enum QueryError {
    #[error("empty expression")]
    EmptyExpression,
    #[error("unexpected atom")]
    UnexpectedAtom,
    #[error("unknown operator `{0}`")]
    UnknownOperator(String),
    #[error("unknown field `{0}`")]
    UnknownField(String),
    #[error("operator `{operator:?}` not allowed for field `{field}`")]
    UnsupportedOperator {
        field: String,
        operator: ComparisonOp,
    },
    #[error("logical operator `{operator:?}` requires at least {expected} operand(s)")]
    InvalidLogicalArity {
        operator: LogicalOp,
        expected: usize,
    },
    #[error("comparison operator `{operator:?}` requires at least one value")]
    MissingComparisonValue { operator: ComparisonOp },
    #[error("field `{field}` does not support sorting")]
    UnsortableField { field: String },
    #[error("type mismatch for field `{field}` and operator `{operator:?}`")]
    TypeMismatch {
        field: String,
        operator: ComparisonOp,
    },
    #[error("invalid cursor `{0}`")]
    InvalidCursor(String),
    #[error("cursor targets stale generation {expected}, current {actual:?}")]
    StaleGeneration { expected: u64, actual: Option<u64> },
    #[error("failed to parse literal `{literal}` as timestamp")]
    InvalidTimestamp { literal: String },
}

pub type QueryResult<T> = std::result::Result<T, QueryError>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ComparisonOp {
    Eq,
    NotEq,
    GreaterThan,
    LessThan,
    GreaterThanOrEq,
    LessThanOrEq,
    In,
    Contains,
}

impl ComparisonOp {
    fn from_str(op: &str) -> Option<Self> {
        match op {
            "=" => Some(Self::Eq),
            "!=" => Some(Self::NotEq),
            ">" => Some(Self::GreaterThan),
            "<" => Some(Self::LessThan),
            ">=" => Some(Self::GreaterThanOrEq),
            "<=" => Some(Self::LessThanOrEq),
            "in" => Some(Self::In),
            "contains" => Some(Self::Contains),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LogicalOp {
    And,
    Or,
    Not,
}

impl LogicalOp {
    fn from_str(op: &str) -> Option<Self> {
        match op {
            "and" => Some(Self::And),
            "or" => Some(Self::Or),
            "not" => Some(Self::Not),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComparisonExpr {
    pub operator: ComparisonOp,
    pub field: String,
    pub values: Vec<Literal>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueryExpr {
    Comparison(ComparisonExpr),
    Logical(LogicalExpr),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LogicalExpr {
    And(Vec<QueryExpr>),
    Or(Vec<QueryExpr>),
    Not(Box<QueryExpr>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Literal {
    String(String),
    Raw(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FieldType {
    String,
    Timestamp,
    StringSet,
    Boolean,
}

#[derive(Debug, Clone)]
pub struct FieldMetadata {
    pub field_type: FieldType,
    pub sortable: bool,
    pub allowed_operators: BTreeSet<ComparisonOp>,
}

#[derive(Debug, Default, Clone)]
pub struct QuerySchema {
    fields: HashMap<String, FieldMetadata>,
}

impl QuerySchema {
    #[must_use]
    pub fn builder() -> QuerySchemaBuilder {
        QuerySchemaBuilder {
            schema: Self {
                fields: HashMap::new(),
            },
        }
    }

    #[must_use]
    pub fn field(&self, name: &str) -> Option<&FieldMetadata> {
        self.fields.get(name)
    }

    /// Validate that the provided query expression conforms to the schema.
    ///
    /// # Errors
    ///
    /// Returns an error when the expression references unknown fields, uses unsupported operators,
    /// or violates logical arity rules.
    pub fn validate_expr(&self, expr: &QueryExpr) -> QueryResult<()> {
        match expr {
            QueryExpr::Comparison(comp) => {
                let field = self
                    .field(&comp.field)
                    .ok_or_else(|| QueryError::UnknownField(comp.field.clone()))?;
                if !field.allowed_operators.contains(&comp.operator) {
                    return Err(QueryError::UnsupportedOperator {
                        field: comp.field.clone(),
                        operator: comp.operator,
                    });
                }
                if comp.values.is_empty() {
                    return Err(QueryError::MissingComparisonValue {
                        operator: comp.operator,
                    });
                }
                Ok(())
            }
            QueryExpr::Logical(logical) => match logical {
                LogicalExpr::And(args) | LogicalExpr::Or(args) => {
                    if args.len() < 2 {
                        return Err(QueryError::InvalidLogicalArity {
                            operator: match logical {
                                LogicalExpr::And(_) => LogicalOp::And,
                                LogicalExpr::Or(_) => LogicalOp::Or,
                                LogicalExpr::Not(_) => unreachable!(),
                            },
                            expected: 2,
                        });
                    }
                    for arg in args {
                        self.validate_expr(arg)?;
                    }
                    Ok(())
                }
                LogicalExpr::Not(expr) => {
                    self.validate_expr(expr)?;
                    Ok(())
                }
            },
        }
    }

    /// Ensure that the given field can be used as a sort key.
    ///
    /// # Errors
    ///
    /// Returns an error when the field is unknown or has not been marked as sortable.
    pub fn ensure_sortable(&self, field: &str) -> QueryResult<()> {
        let metadata = self
            .field(field)
            .ok_or_else(|| QueryError::UnknownField(field.to_string()))?;
        if !metadata.sortable {
            return Err(QueryError::UnsortableField {
                field: field.to_string(),
            });
        }
        Ok(())
    }
}

pub struct QuerySchemaBuilder {
    schema: QuerySchema,
}

impl QuerySchemaBuilder {
    #[must_use]
    pub fn string_field(
        mut self,
        name: impl Into<String>,
        sortable: bool,
        operators: &[ComparisonOp],
    ) -> Self {
        self.schema.fields.insert(
            name.into(),
            FieldMetadata {
                field_type: FieldType::String,
                sortable,
                allowed_operators: operators.iter().copied().collect(),
            },
        );
        self
    }

    #[must_use]
    pub fn timestamp_field(
        mut self,
        name: impl Into<String>,
        sortable: bool,
        operators: &[ComparisonOp],
    ) -> Self {
        self.schema.fields.insert(
            name.into(),
            FieldMetadata {
                field_type: FieldType::Timestamp,
                sortable,
                allowed_operators: operators.iter().copied().collect(),
            },
        );
        self
    }

    #[must_use]
    pub fn string_set_field(
        mut self,
        name: impl Into<String>,
        sortable: bool,
        operators: &[ComparisonOp],
    ) -> Self {
        self.schema.fields.insert(
            name.into(),
            FieldMetadata {
                field_type: FieldType::StringSet,
                sortable,
                allowed_operators: operators.iter().copied().collect(),
            },
        );
        self
    }

    #[must_use]
    pub fn boolean_field(
        mut self,
        name: impl Into<String>,
        sortable: bool,
        operators: &[ComparisonOp],
    ) -> Self {
        self.schema.fields.insert(
            name.into(),
            FieldMetadata {
                field_type: FieldType::Boolean,
                sortable,
                allowed_operators: operators.iter().copied().collect(),
            },
        );
        self
    }

    #[must_use]
    pub fn build(self) -> QuerySchema {
        self.schema
    }
}

#[derive(Debug, Clone)]
pub enum QueryValue {
    String(String),
    Timestamp(LamportTimestamp),
    StringList(Vec<String>),
    Boolean(bool),
}

pub trait QueryRecord {
    fn field_value(&self, field: &str) -> Option<QueryValue>;
}

impl QueryRecord for IssueDetails {
    fn field_value(&self, field: &str) -> Option<QueryValue> {
        match field {
            "status" => Some(QueryValue::String(self.status.to_string())),
            "title" => Some(QueryValue::String(self.title.clone())),
            "updated_at" => Some(QueryValue::Timestamp(self.updated_at.clone())),
            "labels" => Some(QueryValue::StringList(
                self.labels.iter().cloned().collect(),
            )),
            "has_comments" => Some(QueryValue::Boolean(!self.comments.is_empty())),
            _ => None,
        }
    }
}

impl QueryRecord for MilestoneDetails {
    fn field_value(&self, field: &str) -> Option<QueryValue> {
        match field {
            "status" => Some(QueryValue::String(self.status.to_string())),
            "title" => Some(QueryValue::String(self.title.clone())),
            "updated_at" => Some(QueryValue::Timestamp(self.updated_at.clone())),
            "labels" => Some(QueryValue::StringList(
                self.labels.iter().cloned().collect(),
            )),
            "has_comments" => Some(QueryValue::Boolean(!self.comments.is_empty())),
            _ => None,
        }
    }
}

#[derive(Debug, Clone)]
pub struct SortSpec {
    pub field: String,
    pub direction: SortDirection,
}

#[derive(Debug, Clone, Copy)]
pub enum SortDirection {
    Asc,
    Desc,
}

#[derive(Debug, Clone)]
pub struct PageCursor {
    pub offset: usize,
    pub generation: Option<u64>,
}

impl PageCursor {
    #[must_use]
    pub fn encode(&self) -> String {
        self.generation.map_or_else(
            || format!("{}", self.offset),
            |generation| format!("{generation}:{}", self.offset),
        )
    }

    /// Parse a page cursor encoded with [`PageCursor::encode`].
    ///
    /// # Errors
    ///
    /// Returns an error when the cursor string is malformed or contains non-numeric segments.
    pub fn parse(value: &str) -> QueryResult<Self> {
        if let Some((generation, offset)) = value.split_once(':') {
            let generation = generation
                .parse::<u64>()
                .map_err(|_| QueryError::InvalidCursor(value.to_string()))?;
            let offset = offset
                .parse::<usize>()
                .map_err(|_| QueryError::InvalidCursor(value.to_string()))?;
            Ok(Self {
                offset,
                generation: Some(generation),
            })
        } else {
            let offset = value
                .parse::<usize>()
                .map_err(|_| QueryError::InvalidCursor(value.to_string()))?;
            Ok(Self {
                offset,
                generation: None,
            })
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct QueryRequest {
    pub filter: Option<QueryExpr>,
    pub sort: Vec<SortSpec>,
    pub limit: Option<usize>,
    pub cursor: Option<PageCursor>,
}

#[derive(Debug, Clone)]
pub struct QueryResponse<T> {
    pub items: Vec<T>,
    pub next_cursor: Option<String>,
    pub generation: Option<CacheGenerationSnapshot>,
}

pub struct QueryEngine {
    schema: QuerySchema,
}

impl QueryEngine {
    #[must_use]
    pub const fn new(schema: QuerySchema) -> Self {
        Self { schema }
    }

    #[must_use]
    pub const fn schema(&self) -> &QuerySchema {
        &self.schema
    }

    /// Execute the query request against the provided records.
    ///
    /// # Errors
    ///
    /// Returns an error when the filter or sort specifications fail validation or when the provided
    /// cursor references a stale cache generation.
    pub fn execute<R, I>(
        &self,
        records: I,
        request: &QueryRequest,
        generation: Option<&CacheGenerationSnapshot>,
    ) -> QueryResult<QueryResponse<R>>
    where
        R: Clone + QueryRecord,
        I: IntoIterator<Item = R>,
    {
        if let Some(expr) = &request.filter {
            self.schema.validate_expr(expr)?;
        }

        for spec in &request.sort {
            self.schema.ensure_sortable(&spec.field)?;
        }

        if let Some(cursor) = &request.cursor
            && let Some(expected_generation) = cursor.generation
        {
            let actual_generation = generation.map(|snapshot| snapshot.generation);
            if actual_generation != Some(expected_generation) {
                return Err(QueryError::StaleGeneration {
                    expected: expected_generation,
                    actual: actual_generation,
                });
            }
        }

        let mut items: Vec<R> = records.into_iter().collect();

        if let Some(expr) = &request.filter {
            items = items
                .into_iter()
                .filter(|record| self.evaluate(record, expr).unwrap_or(false))
                .collect();
        }

        if !request.sort.is_empty() {
            let specs = request.sort.clone();
            let schema = self.schema.clone();
            items.sort_by(|a, b| compare_records(&schema, &specs, a, b).unwrap_or(Ordering::Equal));
        }

        let total = items.len();
        let offset = request.cursor.as_ref().map_or(0, |c| c.offset);
        let limit = request.limit.unwrap_or(50);
        let generation_snapshot = generation.cloned();

        let slice = items
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();

        let next_cursor = if offset + slice.len() < total {
            Some(
                PageCursor {
                    offset: offset + slice.len(),
                    generation: generation.map(|snapshot| snapshot.generation),
                }
                .encode(),
            )
        } else {
            None
        };

        Ok(QueryResponse {
            items: slice,
            next_cursor,
            generation: generation_snapshot,
        })
    }

    fn evaluate(&self, record: &impl QueryRecord, expr: &QueryExpr) -> QueryResult<bool> {
        match expr {
            QueryExpr::Comparison(comp) => {
                let meta = self
                    .schema
                    .field(&comp.field)
                    .ok_or_else(|| QueryError::UnknownField(comp.field.clone()))?;
                let value = record.field_value(&comp.field);
                Self::evaluate_comparison(meta, value, comp)
            }
            QueryExpr::Logical(logical) => match logical {
                LogicalExpr::And(args) => {
                    for arg in args {
                        if !self.evaluate(record, arg)? {
                            return Ok(false);
                        }
                    }
                    Ok(true)
                }
                LogicalExpr::Or(args) => {
                    for arg in args {
                        if self.evaluate(record, arg)? {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }
                LogicalExpr::Not(expr) => Ok(!self.evaluate(record, expr)?),
            },
        }
    }

    fn evaluate_comparison(
        meta: &FieldMetadata,
        value: Option<QueryValue>,
        expr: &ComparisonExpr,
    ) -> QueryResult<bool> {
        match meta.field_type {
            FieldType::String => {
                let actual = match value {
                    Some(QueryValue::String(val)) => val,
                    Some(QueryValue::Boolean(_)) => {
                        return Err(QueryError::TypeMismatch {
                            field: expr.field.clone(),
                            operator: expr.operator,
                        });
                    }
                    Some(QueryValue::Timestamp(_) | QueryValue::StringList(_)) | None => {
                        String::new()
                    }
                };
                Ok(compare_string(&actual, expr))
            }
            FieldType::Timestamp => {
                let actual = match value {
                    Some(QueryValue::Timestamp(val)) => val,
                    None => return Ok(false),
                    _ => {
                        return Err(QueryError::TypeMismatch {
                            field: expr.field.clone(),
                            operator: expr.operator,
                        });
                    }
                };
                compare_timestamp(&actual, expr)
            }
            FieldType::StringSet => {
                let actual = match value {
                    Some(QueryValue::StringList(list)) => list,
                    None => Vec::new(),
                    _ => {
                        return Err(QueryError::TypeMismatch {
                            field: expr.field.clone(),
                            operator: expr.operator,
                        });
                    }
                };
                Ok(compare_string_list(&actual, expr))
            }
            FieldType::Boolean => {
                let Some(actual) = (match value {
                    Some(QueryValue::Boolean(val)) => Some(val),
                    _ => None,
                }) else {
                    return Err(QueryError::TypeMismatch {
                        field: expr.field.clone(),
                        operator: expr.operator,
                    });
                };
                Ok(compare_boolean(actual, expr))
            }
        }
    }
}

fn compare_records<R: QueryRecord>(
    schema: &QuerySchema,
    specs: &[SortSpec],
    a: &R,
    b: &R,
) -> Option<Ordering> {
    for spec in specs {
        let meta = schema.field(&spec.field)?;
        let ordering = match meta.field_type {
            FieldType::String => {
                let left = match a.field_value(&spec.field) {
                    Some(QueryValue::String(val)) => val,
                    _ => String::new(),
                };
                let right = match b.field_value(&spec.field) {
                    Some(QueryValue::String(val)) => val,
                    _ => String::new(),
                };
                left.cmp(&right)
            }
            FieldType::Timestamp => {
                let left = match a.field_value(&spec.field) {
                    Some(QueryValue::Timestamp(ts)) => ts,
                    _ => LamportTimestamp::new(0, ReplicaId::new("")),
                };
                let right = match b.field_value(&spec.field) {
                    Some(QueryValue::Timestamp(ts)) => ts,
                    _ => LamportTimestamp::new(0, ReplicaId::new("")),
                };
                left.cmp(&right)
            }
            FieldType::Boolean => {
                let left = matches!(a.field_value(&spec.field), Some(QueryValue::Boolean(true)));
                let right = matches!(b.field_value(&spec.field), Some(QueryValue::Boolean(true)));
                left.cmp(&right)
            }
            FieldType::StringSet => Ordering::Equal,
        };

        if ordering != Ordering::Equal {
            return Some(match spec.direction {
                SortDirection::Asc => ordering,
                SortDirection::Desc => ordering.reverse(),
            });
        }
    }

    Some(Ordering::Equal)
}

fn compare_string(value: &str, expr: &ComparisonExpr) -> bool {
    let first = expr.values.first().expect("validated to exist");
    let literal = literal_to_string(first);
    match expr.operator {
        ComparisonOp::Eq => value == literal,
        ComparisonOp::NotEq => value != literal,
        ComparisonOp::Contains => value.contains(literal),
        ComparisonOp::In => expr
            .values
            .iter()
            .map(literal_to_string)
            .any(|candidate| candidate == value),
        ComparisonOp::GreaterThan => value > literal,
        ComparisonOp::LessThan => value < literal,
        ComparisonOp::GreaterThanOrEq => value >= literal,
        ComparisonOp::LessThanOrEq => value <= literal,
    }
}

fn compare_timestamp(value: &LamportTimestamp, expr: &ComparisonExpr) -> QueryResult<bool> {
    let target = parse_lamport(expr.values.first().expect("validated to exist"))?;
    let ordering = value.cmp(&target);
    let result = match expr.operator {
        ComparisonOp::Eq => ordering == Ordering::Equal,
        ComparisonOp::NotEq => ordering != Ordering::Equal,
        ComparisonOp::GreaterThan => ordering == Ordering::Greater,
        ComparisonOp::LessThan => ordering == Ordering::Less,
        ComparisonOp::GreaterThanOrEq => ordering != Ordering::Less,
        ComparisonOp::LessThanOrEq => ordering != Ordering::Greater,
        ComparisonOp::In => expr
            .values
            .iter()
            .filter_map(|lit| parse_lamport(lit).ok())
            .any(|candidate| value == &candidate),
        ComparisonOp::Contains => false,
    };
    Ok(result)
}

fn compare_string_list(values: &[String], expr: &ComparisonExpr) -> bool {
    let needles: Vec<String> = expr
        .values
        .iter()
        .map(|literal| literal_to_string(literal).to_string())
        .collect();
    match expr.operator {
        ComparisonOp::Contains => needles
            .iter()
            .all(|needle| values.iter().any(|candidate| candidate == needle)),
        ComparisonOp::Eq => needles
            .iter()
            .all(|needle| values.iter().any(|candidate| candidate == needle)),
        ComparisonOp::In => needles
            .iter()
            .any(|needle| values.iter().any(|candidate| candidate == needle)),
        ComparisonOp::NotEq => needles
            .iter()
            .all(|needle| values.iter().all(|candidate| candidate != needle)),
        _ => false,
    }
}

fn compare_boolean(value: bool, expr: &ComparisonExpr) -> bool {
    let literal = literal_to_string(expr.values.first().expect("validated"));
    let target = matches!(literal, "true" | "1" | "yes" | "on");
    match expr.operator {
        ComparisonOp::Eq => value == target,
        ComparisonOp::NotEq => value != target,
        _ => false,
    }
}

#[allow(clippy::missing_const_for_fn)]
fn literal_to_string(literal: &Literal) -> &str {
    match literal {
        Literal::String(value) | Literal::Raw(value) => value.as_str(),
    }
}

fn parse_lamport(literal: &Literal) -> QueryResult<LamportTimestamp> {
    let value = literal_to_string(literal);
    crate::dag::OperationId::from_str(value)
        .map(LamportTimestamp::from)
        .map_err(|_| QueryError::InvalidTimestamp {
            literal: value.to_string(),
        })
}

#[derive(Debug, Clone, PartialEq)]
enum SExpr {
    Atom(String),
    List(Vec<SExpr>),
}

/// Parse the textual query expression into an abstract syntax tree.
///
/// # Errors
///
/// Returns an error when the input is empty or cannot be tokenized into a valid expression.
pub fn parse_query(input: &str) -> QueryResult<QueryExpr> {
    let start = Instant::now();
    let (_, expr) = delimited(multispace0, parse_sexpr, multispace0)(input)
        .map_err(|_| QueryError::EmptyExpression)?;
    let parsed = build_expr(expr);
    metrics::histogram!("query.ast_parse_time").record(start.elapsed().as_secs_f64());
    parsed
}

fn parse_sexpr(input: &str) -> nom::IResult<&str, SExpr> {
    alt((parse_list, parse_atom))(input)
}

fn parse_list(input: &str) -> nom::IResult<&str, SExpr> {
    let (input, items) = delimited(
        preceded(multispace0, char('(')),
        many0(preceded(multispace0, parse_sexpr)),
        preceded(multispace0, char(')')),
    )(input)?;
    Ok((input, SExpr::List(items)))
}

fn parse_atom(input: &str) -> nom::IResult<&str, SExpr> {
    let quoted = delimited(
        char('"'),
        escaped_transform(is_not("\"\\"), '\\', alt((tag("\""), tag("\\"), tag("n")))),
        char('"'),
    );
    let bare = take_while1(|c: char| !c.is_whitespace() && c != '(' && c != ')');
    map(
        alt((quoted, map(bare, |s: &str| s.to_string()))),
        SExpr::Atom,
    )(input)
}

fn build_expr(expr: SExpr) -> QueryResult<QueryExpr> {
    match expr {
        SExpr::Atom(_) => Err(QueryError::UnexpectedAtom),
        SExpr::List(items) => {
            let mut parts = items.into_iter();
            let Some(operator_expr) = parts.next() else {
                return Err(QueryError::EmptyExpression);
            };
            let SExpr::Atom(operator) = operator_expr else {
                return Err(QueryError::UnexpectedAtom);
            };

            let remaining: Vec<SExpr> = parts.collect();

            if let Some(logical) = LogicalOp::from_str(&operator) {
                return build_logical(logical, remaining);
            }

            if let Some(comparison) = ComparisonOp::from_str(&operator) {
                return build_comparison(comparison, remaining);
            }

            Err(QueryError::UnknownOperator(operator))
        }
    }
}

fn build_logical(operator: LogicalOp, items: Vec<SExpr>) -> QueryResult<QueryExpr> {
    match operator {
        LogicalOp::Not => {
            if items.len() != 1 {
                return Err(QueryError::InvalidLogicalArity {
                    operator,
                    expected: 1,
                });
            }
            let mut iter = items.into_iter();
            let Some(item) = iter.next() else {
                return Err(QueryError::EmptyExpression);
            };
            let expr = build_expr(item)?;
            Ok(QueryExpr::Logical(LogicalExpr::Not(Box::new(expr))))
        }
        LogicalOp::And | LogicalOp::Or => {
            if items.len() < 2 {
                return Err(QueryError::InvalidLogicalArity {
                    operator,
                    expected: 2,
                });
            }
            let mut args = Vec::with_capacity(items.len());
            for item in items {
                args.push(build_expr(item)?);
            }
            Ok(QueryExpr::Logical(match operator {
                LogicalOp::And => LogicalExpr::And(args),
                LogicalOp::Or => LogicalExpr::Or(args),
                LogicalOp::Not => unreachable!(),
            }))
        }
    }
}

fn build_comparison(operator: ComparisonOp, mut items: Vec<SExpr>) -> QueryResult<QueryExpr> {
    if items.len() < 2 {
        return Err(QueryError::MissingComparisonValue { operator });
    }
    let field_expr = items.remove(0);
    let field = match field_expr {
        SExpr::Atom(atom) => atom,
        SExpr::List(_) => return Err(QueryError::UnexpectedAtom),
    };
    let mut values = Vec::with_capacity(items.len());
    for literal in items {
        match literal {
            SExpr::Atom(atom) => values.push(Literal::String(atom)),
            SExpr::List(_) => return Err(QueryError::UnexpectedAtom),
        }
    }

    Ok(QueryExpr::Comparison(ComparisonExpr {
        operator,
        field,
        values,
    }))
}

#[must_use]
pub fn milestone_schema() -> QuerySchema {
    QuerySchema::builder()
        .string_field(
            "status",
            true,
            &[ComparisonOp::Eq, ComparisonOp::NotEq, ComparisonOp::In],
        )
        .string_field(
            "title",
            true,
            &[
                ComparisonOp::Eq,
                ComparisonOp::NotEq,
                ComparisonOp::Contains,
            ],
        )
        .timestamp_field(
            "updated_at",
            true,
            &[
                ComparisonOp::Eq,
                ComparisonOp::NotEq,
                ComparisonOp::GreaterThan,
                ComparisonOp::LessThan,
                ComparisonOp::GreaterThanOrEq,
                ComparisonOp::LessThanOrEq,
            ],
        )
        .string_set_field(
            "labels",
            false,
            &[
                ComparisonOp::Contains,
                ComparisonOp::Eq,
                ComparisonOp::NotEq,
                ComparisonOp::In,
            ],
        )
        .boolean_field(
            "has_comments",
            true,
            &[ComparisonOp::Eq, ComparisonOp::NotEq],
        )
        .build()
}

#[must_use]
pub fn issue_schema() -> QuerySchema {
    QuerySchema::builder()
        .string_field(
            "status",
            true,
            &[ComparisonOp::Eq, ComparisonOp::NotEq, ComparisonOp::In],
        )
        .string_field(
            "title",
            true,
            &[
                ComparisonOp::Eq,
                ComparisonOp::NotEq,
                ComparisonOp::Contains,
            ],
        )
        .timestamp_field(
            "updated_at",
            true,
            &[
                ComparisonOp::Eq,
                ComparisonOp::NotEq,
                ComparisonOp::GreaterThan,
                ComparisonOp::LessThan,
                ComparisonOp::GreaterThanOrEq,
                ComparisonOp::LessThanOrEq,
            ],
        )
        .string_set_field(
            "labels",
            false,
            &[
                ComparisonOp::Contains,
                ComparisonOp::Eq,
                ComparisonOp::NotEq,
                ComparisonOp::In,
            ],
        )
        .boolean_field(
            "has_comments",
            true,
            &[ComparisonOp::Eq, ComparisonOp::NotEq],
        )
        .build()
}

/// Parse and validate an optional filter string against the provided schema.
///
/// # Errors
///
/// Returns an error when the filter string cannot be parsed or fails schema validation.
pub fn prepare_filter(schema: &QuerySchema, input: Option<&str>) -> QueryResult<Option<QueryExpr>> {
    match input {
        Some(raw) if !raw.trim().is_empty() => {
            let expr = parse_query(raw)?;
            schema.validate_expr(&expr)?;
            Ok(Some(expr))
        }
        _ => Ok(None),
    }
}

/// Parse CLI-style sort specifications (`field[:asc|desc]`) into structured definitions.
///
/// # Errors
///
/// Returns an error when a token is malformed or references an unknown sort direction.
pub fn parse_sort_specs(input: &[String]) -> QueryResult<Vec<SortSpec>> {
    let mut specs = Vec::with_capacity(input.len());
    for token in input {
        let parts: Vec<&str> = token.split(':').collect();
        let field = parts
            .first()
            .ok_or_else(|| QueryError::UnknownField(token.clone()))?
            .trim()
            .to_string();
        let direction = match parts.get(1).map(|s| s.to_ascii_lowercase()) {
            Some(dir) if dir == "desc" => SortDirection::Desc,
            Some(dir) if dir == "asc" => SortDirection::Asc,
            Some(_) => {
                return Err(QueryError::UnknownOperator(token.clone()));
            }
            None => SortDirection::Asc,
        };
        specs.push(SortSpec { field, direction });
    }
    Ok(specs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::issue::IssueStatus;
    use crate::mile::MileStatus;
    use crate::model::{Comment, CommentParent, Markdown};

    fn sample_issue() -> IssueDetails {
        let id = crate::issue::IssueId::new();
        let comment = Comment {
            id: crate::model::CommentId::new_v4(),
            parent: CommentParent::Issue(id.clone()),
            body_markdown: Markdown::new("Investigate"),
            author_id: "alice".into(),
            created_at: LamportTimestamp::new(1, ReplicaId::new("alice")),
            edited_at: None,
        };
        IssueDetails {
            id,
            title: "Fix bug".into(),
            description: None,
            status: IssueStatus::Open,
            initial_comment_id: Some(comment.id),
            labels: std::iter::once("bug".to_string()).collect(),
            comments: vec![comment],
            label_events: vec![],
            created_at: LamportTimestamp::new(1, ReplicaId::new("alice")),
            updated_at: LamportTimestamp::new(2, ReplicaId::new("alice")),
            clock_snapshot: LamportTimestamp::new(2, ReplicaId::new("alice")),
        }
    }

    fn sample_milestone() -> MilestoneDetails {
        let id = crate::mile::MileId::new();
        let comment = Comment {
            id: crate::model::CommentId::new_v4(),
            parent: CommentParent::Milestone(id.clone()),
            body_markdown: Markdown::new("Kickoff"),
            author_id: "alice".into(),
            created_at: LamportTimestamp::new(1, ReplicaId::new("alice")),
            edited_at: None,
        };
        MilestoneDetails {
            id,
            title: "Milestone A".into(),
            description: None,
            status: MileStatus::Open,
            initial_comment_id: Some(comment.id),
            labels: std::iter::once("release".to_string()).collect(),
            comments: vec![comment],
            label_events: vec![],
            created_at: LamportTimestamp::new(1, ReplicaId::new("alice")),
            updated_at: LamportTimestamp::new(3, ReplicaId::new("alice")),
            clock_snapshot: LamportTimestamp::new(3, ReplicaId::new("alice")),
        }
    }

    #[test]
    fn parses_simple_comparison() {
        let expr = parse_query("(= status \"open\")").expect("parse");
        match expr {
            QueryExpr::Comparison(comp) => {
                assert_eq!(comp.operator, ComparisonOp::Eq);
                assert_eq!(comp.field, "status");
                assert_eq!(comp.values, vec![Literal::String("open".to_string())]);
            }
            QueryExpr::Logical(other) => panic!("expected comparison, got logical {other:?}"),
        }
    }

    #[test]
    fn parses_logical_expression() {
        let expr =
            parse_query("(and (= status \"open\") (contains title \"Fix\"))").expect("parse");
        match expr {
            QueryExpr::Logical(LogicalExpr::And(args)) => {
                assert_eq!(args.len(), 2);
            }
            QueryExpr::Logical(other) => panic!("expected logical and, got {other:?}"),
            QueryExpr::Comparison(other) => {
                panic!("expected logical expression, got comparison {other:?}")
            }
        }
    }

    #[test]
    fn evaluates_issue_filter() {
        let engine = QueryEngine::new(issue_schema());
        let request = QueryRequest {
            filter: Some(parse_query("(= status \"open\")").unwrap()),
            sort: vec![],
            limit: Some(10),
            cursor: None,
        };
        let issues = vec![sample_issue()];
        let response = engine.execute(issues.clone(), &request, None).unwrap();
        assert_eq!(response.items.len(), 1);

        let closed = parse_query("(= status \"closed\")").unwrap();
        engine.schema().validate_expr(&closed).unwrap();
        let request = QueryRequest {
            filter: Some(closed),
            sort: vec![],
            limit: Some(10),
            cursor: None,
        };
        let response = engine.execute(issues, &request, None).unwrap();
        assert!(response.items.is_empty());
    }

    #[test]
    fn sort_by_updated_at_descending() {
        let schema = milestone_schema();
        let engine = QueryEngine::new(schema);
        let mut first = sample_milestone();
        first.updated_at = LamportTimestamp::new(5, ReplicaId::new("a"));
        let mut second = sample_milestone();
        second.title = "Later".into();
        second.updated_at = LamportTimestamp::new(7, ReplicaId::new("b"));

        let request = QueryRequest {
            filter: None,
            sort: vec![SortSpec {
                field: "updated_at".into(),
                direction: SortDirection::Desc,
            }],
            limit: Some(10),
            cursor: None,
        };

        let response = engine
            .execute(vec![first.clone(), second.clone()], &request, None)
            .unwrap();
        assert_eq!(response.items[0].title, second.title);
        assert_eq!(response.items[1].title, first.title);
    }

    #[test]
    fn cursor_roundtrips_generation() {
        let cursor = PageCursor {
            offset: 42,
            generation: Some(7),
        };
        let encoded = cursor.encode();
        assert_eq!(encoded, "7:42");
        let parsed = PageCursor::parse(&encoded).expect("parse cursor");
        assert_eq!(parsed.offset, 42);
        assert_eq!(parsed.generation, Some(7));
    }

    #[test]
    fn execute_rejects_stale_generation_cursor() {
        let engine = QueryEngine::new(issue_schema());
        let cursor = PageCursor {
            offset: 0,
            generation: Some(3),
        };
        let request = QueryRequest {
            filter: None,
            sort: vec![],
            limit: Some(10),
            cursor: Some(cursor),
        };
        let generation = CacheGenerationSnapshot {
            generation: 4,
            created_at: 0,
            base_clock: None,
        };
        let result = engine.execute(vec![sample_issue()], &request, Some(&generation));
        assert!(matches!(
            result,
            Err(QueryError::StaleGeneration {
                expected: 3,
                actual: Some(4)
            })
        ));
    }

    #[test]
    fn next_cursor_includes_generation_when_available() {
        let schema = issue_schema();
        let engine = QueryEngine::new(schema);
        let issues = vec![sample_issue(), sample_issue(), sample_issue()];
        let request = QueryRequest {
            filter: None,
            sort: vec![],
            limit: Some(2),
            cursor: None,
        };
        let generation = CacheGenerationSnapshot {
            generation: 11,
            created_at: 123,
            base_clock: None,
        };
        let response = engine.execute(issues, &request, Some(&generation)).unwrap();
        assert_eq!(response.generation.as_ref().map(|g| g.generation), Some(11));
        assert_eq!(response.next_cursor.as_deref(), Some("11:2"));
    }
}
