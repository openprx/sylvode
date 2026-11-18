#![allow(dead_code)]

pub mod activity;
pub mod ai_learning_record;
pub mod ai_participant;
pub mod appeal;
pub mod comment;
pub mod decision;
pub mod decision_audit_report;
pub mod decision_domain;
pub mod feedback_loop_link;
pub mod governance_audit_log;
pub mod governance_config;
pub mod impact_metric;
pub mod impact_review;
pub mod label;
pub mod notification;
pub mod project;
pub mod proposal;
pub mod proposal_comment;
pub mod proposal_issue_link;
pub mod proposal_template;
pub mod review_participant;
pub mod sprint;
pub mod trust_score;
pub mod trust_score_log;
pub mod user;
pub mod veto_event;
pub mod vetoer;
pub mod vote;
pub mod webhook;
pub mod work_item;
pub mod work_item_label;
pub mod workspace;
pub mod workspace_member;

// The API represents SQL dates as ISO calendar dates, without a time or offset.
pub(crate) const ISO_DATE_FORMAT: time::format_description::StaticFormatDescription =
    time::macros::format_description!("[year]-[month]-[day]");
time::serde::format_description!(pub(crate) iso_date, Date, super::ISO_DATE_FORMAT);
