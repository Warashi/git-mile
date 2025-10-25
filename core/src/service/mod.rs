pub mod issues;
pub mod milestones;

pub use issues::{
    AppendCommentPayload as IssueAppendCommentPayload, CreatePayload as IssueCreatePayload,
    IssueService, LabelUpdatePayload as IssueLabelUpdatePayload,
};
pub use milestones::{
    AppendCommentPayload as MileAppendCommentPayload, CreatePayload as MileCreatePayload,
    LabelUpdatePayload as MileLabelUpdatePayload, MilestoneService,
};
