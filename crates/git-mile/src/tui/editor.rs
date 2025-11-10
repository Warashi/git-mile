use std::collections::BTreeSet;
use std::str::FromStr;

use anyhow::Result;
use git_mile_core::id::TaskId;
use git_mile_core::{StateKindFilter, TaskFilter};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

use crate::filter_util::{TaskFilterBuilder, normalize_timestamp, parse_timestamp};

use super::app::NewTaskData;
use crate::task_cache::TaskView;

pub(super) fn comment_editor_template(actor: &git_mile_core::event::Actor, task: TaskId) -> String {
    format!(
        "# コメントを入力してください。\n# 空のまま保存するとキャンセルされます。\n# Task: {task}\n# Actor: {} <{}>\n\n",
        actor.name, actor.email
    )
}

pub(super) fn parse_comment_editor_output(raw: &str) -> Option<String> {
    let body = raw
        .lines()
        .filter(|line| !line.trim_start().starts_with('#'))
        .collect::<Vec<_>>()
        .join("\n");
    let trimmed = body.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_owned())
    }
}

pub(super) fn edit_task_editor_template(task: &TaskView, state_hint: Option<&str>) -> String {
    let snapshot = &task.snapshot;
    let state = snapshot.state.as_deref().unwrap_or_default();
    let labels = if snapshot.labels.is_empty() {
        String::new()
    } else {
        snapshot
            .labels
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };
    let assignees = if snapshot.assignees.is_empty() {
        String::new()
    } else {
        snapshot
            .assignees
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>()
            .join(", ")
    };

    let mut lines = vec![
        "# 選択中のタスクを編集します。タイトルは必須です。".to_string(),
        "# 空のフィールドは対応する値をクリアします。".to_string(),
        format!("title: {}", snapshot.title),
    ];
    if let Some(hint) = state_hint {
        lines.push(format!("# state 候補: {hint}"));
    }
    lines.extend([
        format!("state: {state}"),
        format!("labels: {labels}"),
        format!("assignees: {assignees}"),
        "---".to_string(),
        "# この下で説明を編集してください。空欄で説明を削除します。".to_string(),
    ]);
    if snapshot.description.is_empty() {
        lines.push(String::new());
    } else {
        lines.extend(snapshot.description.lines().map(str::to_owned));
    }
    lines.push(String::new());
    lines.join("\n")
}

pub(super) fn new_task_editor_template(
    parent: Option<&TaskView>,
    state_hint: Option<&str>,
    default_state: Option<&str>,
) -> String {
    let header = parent.map_or_else(
        || "# 新規タスクを作成します。".to_owned(),
        |p| {
            format!(
                "# 新規タスク（親: {} [{}...]）を作成します。",
                p.snapshot.title,
                &p.snapshot.id.to_string()[..12]
            )
        },
    );

    let mut lines = vec![
        header,
        "# タイトルは必須です。".to_string(),
        "# 空のまま保存すると作成をキャンセルしたものとして扱います。".to_string(),
        "title: ".to_string(),
    ];
    if let Some(hint) = state_hint {
        lines.push(format!("# state 候補: {hint}"));
    }
    let state_line = default_state.map_or_else(|| "state: ".to_string(), |value| format!("state: {value}"));
    lines.extend([
        state_line,
        "labels: ".to_string(),
        "assignees: ".to_string(),
        "---".to_string(),
        "# この下に説明をMarkdown形式で記入してください。不要なら空のままにしてください。".to_string(),
        String::new(),
    ]);
    lines.join("\n")
}

pub(super) fn filter_editor_template(filter: &TaskFilter) -> String {
    let states = filter.states.iter().cloned().collect::<Vec<_>>().join(", ");
    let labels = filter.labels.iter().cloned().collect::<Vec<_>>().join(", ");
    let assignees = filter.assignees.iter().cloned().collect::<Vec<_>>().join(", ");
    let parents = filter
        .parents
        .iter()
        .map(TaskId::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let children = filter
        .children
        .iter()
        .map(TaskId::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let text = filter.text.clone().unwrap_or_default();
    let updated_since = filter
        .updated
        .as_ref()
        .and_then(|updated| updated.since)
        .map(format_timestamp)
        .unwrap_or_default();
    let updated_until = filter
        .updated
        .as_ref()
        .and_then(|updated| updated.until)
        .map(format_timestamp)
        .unwrap_or_default();

    let lines = vec![
        "# フィルタを編集します。空欄のフィールドは該当条件なしとして扱われます。".to_string(),
        "# states/labels/assignees/parents/children はカンマ区切りで入力してください。".to_string(),
        "# updated_since / updated_until は RFC3339 (例: 2025-01-01T09:00:00+09:00) 形式。".to_string(),
        "# state_kinds には done/in_progress などの kind を指定し、!done で除外できます。".to_string(),
        format!("# state_kinds の候補: {}", state_kind_options_hint()),
        format!("states: {states}"),
        format!(
            "state_kinds: {}",
            state_kind_filter_to_editor_value(&filter.state_kinds)
        ),
        format!("labels: {labels}"),
        format!("assignees: {assignees}"),
        format!("parents: {parents}"),
        format!("children: {children}"),
        format!("text: {text}"),
        format!("updated_since: {updated_since}"),
        format!("updated_until: {updated_until}"),
        String::new(),
    ];
    lines.join("\n")
}

pub(super) fn state_kind_filter_to_editor_value(filter: &StateKindFilter) -> String {
    if filter.is_empty() {
        return String::new();
    }
    let mut tokens = Vec::new();
    for kind in &filter.include {
        tokens.push(kind.as_str().to_string());
    }
    for kind in &filter.exclude {
        tokens.push(format!("!{}", kind.as_str()));
    }
    tokens.join(", ")
}

pub(super) fn state_kind_summary_tokens(filter: &StateKindFilter) -> Vec<String> {
    if filter.is_empty() {
        return Vec::new();
    }
    let mut tokens = Vec::new();
    if !filter.include.is_empty() {
        let includes = filter
            .include
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        tokens.push(format!("state-kind={includes}"));
    }
    if !filter.exclude.is_empty() {
        let excludes = filter
            .exclude
            .iter()
            .map(|kind| kind.as_str())
            .collect::<Vec<_>>()
            .join(", ");
        tokens.push(format!("state-kind!={excludes}"));
    }
    tokens
}

pub(super) fn parse_new_task_editor_output(raw: &str) -> Result<Option<NewTaskData>, String> {
    let mut title: Option<&str> = None;
    let mut state: Option<&str> = None;
    let mut labels: Option<&str> = None;
    let mut assignees: Option<&str> = None;
    let mut description_lines = Vec::new();
    let mut in_description = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('#') {
            continue;
        }
        if in_description {
            description_lines.push(line);
            continue;
        }

        if trimmed.is_empty() {
            continue;
        }
        if trimmed == "---" {
            in_description = true;
            continue;
        }
        if let Some((key, value)) = trimmed.split_once(':') {
            let value = value.trim();
            match key.trim() {
                "title" => title = Some(value),
                "state" => state = Some(value),
                "labels" => labels = Some(value),
                "assignees" => assignees = Some(value),
                unknown => {
                    return Err(format!("未知のフィールドです: {unknown}"));
                }
            }
        } else {
            return Err(format!("フィールドの形式が正しくありません: {trimmed}"));
        }
    }

    let title = title.unwrap_or("").trim();
    let state = state.unwrap_or("").trim();
    let labels = labels.unwrap_or("").trim();
    let assignees = assignees.unwrap_or("").trim();
    let description = description_lines.join("\n");

    let is_all_empty = title.is_empty()
        && state.is_empty()
        && labels.is_empty()
        && assignees.is_empty()
        && description.trim().is_empty();
    if is_all_empty {
        return Ok(None);
    }

    if title.is_empty() {
        return Err("タイトルを入力してください".into());
    }

    let state = if state.is_empty() {
        None
    } else {
        Some(state.to_owned())
    };
    let labels = parse_list(labels);
    let assignees = parse_list(assignees);
    let description = if description.trim().is_empty() {
        None
    } else {
        Some(description.trim_end().to_owned())
    };

    Ok(Some(NewTaskData {
        title: title.to_owned(),
        state,
        labels,
        assignees,
        description,
        parent: None,
    }))
}

pub(super) fn parse_filter_editor_output(raw: &str) -> Result<TaskFilter, String> {
    let mut states = BTreeSet::new();
    let mut labels = BTreeSet::new();
    let mut assignees = BTreeSet::new();
    let mut parents = BTreeSet::new();
    let mut children = BTreeSet::new();
    let mut text: Option<String> = None;
    let mut updated_since: Option<OffsetDateTime> = None;
    let mut updated_until: Option<OffsetDateTime> = None;
    let mut include_state_kind_tokens: Vec<String> = Vec::new();
    let mut exclude_state_kind_tokens: Vec<String> = Vec::new();

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        let Some((key, value)) = trimmed.split_once(':') else {
            return Err(format!("フィールドの形式が正しくありません: {trimmed}"));
        };
        let key = key.trim();
        let value = value.trim();
        match key {
            "states" => {
                states = parse_list(value).into_iter().collect();
            }
            "labels" => {
                labels = parse_list(value).into_iter().collect();
            }
            "assignees" => {
                assignees = parse_list(value).into_iter().collect();
            }
            "parents" => {
                parents = parse_task_id_list(value)?;
            }
            "children" => {
                children = parse_task_id_list(value)?;
            }
            "text" => {
                if value.is_empty() {
                    text = None;
                } else {
                    text = Some(value.to_owned());
                }
            }
            "updated_since" => {
                updated_since = parse_optional_timestamp(value)?;
            }
            "updated_until" => {
                updated_until = parse_optional_timestamp(value)?;
            }
            "state_kinds" => {
                (include_state_kind_tokens, exclude_state_kind_tokens) = split_state_kind_tokens(value);
            }
            unknown => return Err(format!("未知のフィールドです: {unknown}")),
        }
    }

    let states_vec: Vec<String> = states.into_iter().collect();
    let labels_vec: Vec<String> = labels.into_iter().collect();
    let assignees_vec: Vec<String> = assignees.into_iter().collect();
    let parents_vec: Vec<TaskId> = parents.into_iter().collect();
    let children_vec: Vec<TaskId> = children.into_iter().collect();
    let text_input = text;

    let mut builder = TaskFilterBuilder::new()
        .with_states(&states_vec)
        .with_labels(&labels_vec)
        .with_assignees(&assignees_vec)
        .with_parents(&parents_vec)
        .with_children(&children_vec)
        .with_text(text_input);

    builder = builder
        .with_state_kinds(&include_state_kind_tokens, &exclude_state_kind_tokens)
        .map_err(|err| err.describe_user_facing())?;
    builder = builder.with_time_range_values(updated_since, updated_until);

    Ok(builder.build())
}

fn parse_task_id_list(input: &str) -> Result<BTreeSet<TaskId>, String> {
    let mut ids = BTreeSet::new();
    for value in parse_list(input) {
        let id = TaskId::from_str(&value).map_err(|_| format!("TaskId の形式が正しくありません: {value}"))?;
        ids.insert(id);
    }
    Ok(ids)
}

fn parse_optional_timestamp(input: &str) -> Result<Option<OffsetDateTime>, String> {
    if input.is_empty() {
        return Ok(None);
    }
    parse_timestamp(input)
        .map(normalize_timestamp)
        .map(Some)
        .map_err(|err| format!("時刻の形式が正しくありません ({input}): {err}"))
}

fn split_state_kind_tokens(input: &str) -> (Vec<String>, Vec<String>) {
    let mut include = Vec::new();
    let mut exclude = Vec::new();
    for token in parse_list(input) {
        if let Some(rest) = token.strip_prefix('!') {
            exclude.push(rest.to_owned());
        } else {
            include.push(token);
        }
    }
    (include, exclude)
}

const STATE_KIND_HINT: &str = "done, in_progress, blocked, todo, backlog";

pub(super) const fn state_kind_options_hint() -> &'static str {
    STATE_KIND_HINT
}

pub(super) fn parse_list(input: &str) -> Vec<String> {
    input
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_owned)
        .collect()
}

pub(super) fn summarize_task_filter(filter: &TaskFilter) -> String {
    if filter.is_empty() {
        return "未設定".to_string();
    }
    let mut parts = Vec::new();
    if !filter.states.is_empty() {
        parts.push(format!("state={}", join_string_set(&filter.states)));
    }
    if !filter.labels.is_empty() {
        parts.push(format!("label={}", join_string_set(&filter.labels)));
    }
    if !filter.assignees.is_empty() {
        parts.push(format!("assignee={}", join_string_set(&filter.assignees)));
    }
    if !filter.parents.is_empty() {
        parts.push(format!("parent={}", join_task_ids(&filter.parents)));
    }
    if !filter.children.is_empty() {
        parts.push(format!("child={}", join_task_ids(&filter.children)));
    }
    if let Some(text) = filter.text.as_deref().and_then(|value| {
        let trimmed = value.trim();
        (!trimmed.is_empty()).then_some(trimmed)
    }) {
        parts.push(format!("text=\"{text}\""));
    }
    if let Some(updated) = &filter.updated {
        if let Some(since) = updated.since {
            parts.push(format!("since={}", format_timestamp(since)));
        }
        if let Some(until) = updated.until {
            parts.push(format!("until={}", format_timestamp(until)));
        }
    }
    parts.extend(state_kind_summary_tokens(&filter.state_kinds));
    if parts.is_empty() {
        "未設定".into()
    } else {
        parts.join(" / ")
    }
}

fn join_string_set(values: &BTreeSet<String>) -> String {
    values.iter().map(String::as_str).collect::<Vec<_>>().join(", ")
}

fn join_task_ids(values: &BTreeSet<TaskId>) -> String {
    values.iter().map(short_task_id).collect::<Vec<_>>().join(", ")
}

fn short_task_id(task_id: &TaskId) -> String {
    let id = task_id.to_string();
    id.chars().take(8).collect()
}

pub(super) fn format_timestamp(ts: OffsetDateTime) -> String {
    ts.format(&Rfc3339).unwrap_or_else(|_| ts.to_string())
}
