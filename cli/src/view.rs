use git_mile_core::issue::{IssueId, IssueStatus};
use git_mile_core::mile::{MileId, MileStatus};
use git_mile_core::model::{
    Comment as CoreComment, IssueDetails as CoreIssueDetails, LabelEvent as CoreLabelEvent,
    LabelOperation, Markdown as CoreMarkdown, MilestoneDetails as CoreMilestoneDetails,
};
use git_mile_core::{CommentId, LamportTimestamp};
use serde::Serialize;

const PREVIEW_LIMIT: usize = 80;

#[derive(Clone, Debug, Serialize)]
pub struct CommentView {
    pub id: CommentId,
    pub author: String,
    pub created_at: LamportTimestamp,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edited_at: Option<LamportTimestamp>,
    pub body: String,
}

#[derive(Clone, Debug, Serialize)]
pub struct LabelEventView {
    pub operation: LabelOperation,
    pub label: String,
    pub actor: String,
    pub timestamp: LamportTimestamp,
}

#[derive(Clone, Debug, Serialize)]
pub struct IssueDetailsView {
    pub id: IssueId,
    pub title: String,
    pub status: IssueStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_preview: Option<String>,
    pub labels: Vec<String>,
    pub comment_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commented_at: Option<LamportTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_comment_excerpt: Option<String>,
    pub comments: Vec<CommentView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_comment_id: Option<CommentId>,
    pub label_events: Vec<LabelEventView>,
    pub created_at: LamportTimestamp,
    pub updated_at: LamportTimestamp,
    pub clock_snapshot: LamportTimestamp,
}

#[derive(Clone, Debug, Serialize)]
pub struct MilestoneDetailsView {
    pub id: MileId,
    pub title: String,
    pub status: MileStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description_preview: Option<String>,
    pub labels: Vec<String>,
    pub comment_count: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_commented_at: Option<LamportTimestamp>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_comment_excerpt: Option<String>,
    pub comments: Vec<CommentView>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub initial_comment_id: Option<CommentId>,
    pub label_events: Vec<LabelEventView>,
    pub created_at: LamportTimestamp,
    pub updated_at: LamportTimestamp,
    pub clock_snapshot: LamportTimestamp,
}

impl From<CoreComment> for CommentView {
    fn from(comment: CoreComment) -> Self {
        Self {
            id: comment.id,
            author: comment.author_id,
            created_at: comment.created_at,
            edited_at: comment.edited_at,
            body: comment.body_markdown.into_string(),
        }
    }
}

impl From<CoreLabelEvent> for LabelEventView {
    fn from(event: CoreLabelEvent) -> Self {
        Self {
            operation: event.operation,
            label: event.label_id,
            actor: event.actor_id,
            timestamp: event.timestamp,
        }
    }
}

impl From<CoreIssueDetails> for IssueDetailsView {
    fn from(details: CoreIssueDetails) -> Self {
        let CoreIssueDetails {
            id,
            title,
            description,
            status,
            initial_comment_id,
            labels,
            comments,
            label_events,
            created_at,
            updated_at,
            clock_snapshot,
        } = details;

        let description_string = description.map(CoreMarkdown::into_string);
        let description_preview = description_string
            .as_deref()
            .map(|value| make_preview(value, PREVIEW_LIMIT));

        let mut sorted_labels: Vec<String> = labels.into_iter().collect();
        sorted_labels.sort();

        let comment_views: Vec<CommentView> = comments.into_iter().map(CommentView::from).collect();
        let comment_count = comment_views.len();
        let last_commented_at = comment_views.last().map(|comment| comment.created_at.clone());
        let latest_comment_excerpt = comment_views
            .last()
            .map(|comment| make_preview(&comment.body, PREVIEW_LIMIT));

        let label_history: Vec<LabelEventView> =
            label_events.into_iter().map(LabelEventView::from).collect();

        Self {
            id,
            title,
            status,
            description: description_string,
            description_preview,
            labels: sorted_labels,
            comment_count,
            last_commented_at,
            latest_comment_excerpt,
            comments: comment_views,
            initial_comment_id,
            label_events: label_history,
            created_at,
            updated_at,
            clock_snapshot,
        }
    }
}

impl From<CoreMilestoneDetails> for MilestoneDetailsView {
    fn from(details: CoreMilestoneDetails) -> Self {
        let CoreMilestoneDetails {
            id,
            title,
            description,
            status,
            initial_comment_id,
            labels,
            comments,
            label_events,
            created_at,
            updated_at,
            clock_snapshot,
        } = details;

        let description_string = description.map(CoreMarkdown::into_string);
        let description_preview = description_string
            .as_deref()
            .map(|value| make_preview(value, PREVIEW_LIMIT));

        let mut sorted_labels: Vec<String> = labels.into_iter().collect();
        sorted_labels.sort();

        let comment_views: Vec<CommentView> = comments.into_iter().map(CommentView::from).collect();
        let comment_count = comment_views.len();
        let last_commented_at = comment_views.last().map(|comment| comment.created_at.clone());
        let latest_comment_excerpt = comment_views
            .last()
            .map(|comment| make_preview(&comment.body, PREVIEW_LIMIT));

        let label_history: Vec<LabelEventView> =
            label_events.into_iter().map(LabelEventView::from).collect();

        Self {
            id,
            title,
            status,
            description: description_string,
            description_preview,
            labels: sorted_labels,
            comment_count,
            last_commented_at,
            latest_comment_excerpt,
            comments: comment_views,
            initial_comment_id,
            label_events: label_history,
            created_at,
            updated_at,
            clock_snapshot,
        }
    }
}

fn make_preview(original: &str, limit: usize) -> String {
    let normalized = collapse_whitespace(original);
    if normalized.len() <= limit {
        normalized
    } else {
        let mut truncated = String::new();
        for ch in normalized.chars().take(limit) {
            truncated.push(ch);
        }
        truncated.push_str("...");
        truncated
    }
}

fn collapse_whitespace(input: &str) -> String {
    let mut result = String::new();
    let mut last_was_space = false;
    for ch in input.chars() {
        if ch.is_whitespace() {
            if !last_was_space {
                result.push(' ');
                last_was_space = true;
            }
        } else {
            last_was_space = false;
            result.push(ch);
        }
    }
    result.trim().to_string()
}
