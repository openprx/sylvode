//! Mentions in comments: resolving them from the text, storing them, and showing people.
//!
//! A mention is a person picked explicitly (an id the client sends) or typed as `@Name`. Typed
//! names are matched against the names of the people who can be mentioned, longest name first,
//! case-insensitively and exactly; a name shared by two people is not resolved, the client has to
//! pick one. A mention is stored as `{id, label}` where the label is the name it was resolved
//! from. Readers show the person's current name and use the label only for someone who has left.
//!
//! No answer, notification, e-mail text or webhook field meant for display ever carries an id as
//! text: [`Person::name`] is `None` when there is no name, and the client words that case itself.

// The maps are small, built and read inside this API, and never cross a hasher boundary.
#![allow(clippy::implicit_hasher)]

use std::collections::{HashMap, HashSet};

use sea_orm::{ConnectionTrait, DbBackend, FromQueryResult, Statement};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::error::ApiError;

/// Most mentions one comment can store (also enforced by the `mentions` column).
pub const MENTION_CAP: usize = 20;

/// A person who can be mentioned.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Member {
    pub id: Uuid,
    pub name: String,
}

/// A stored mention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Mention {
    pub id: Uuid,
    #[serde(default)]
    pub label: String,
}

/// Whether a person can still be reached in the place the answer is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PersonState {
    Active,
    Removed,
}

/// A person as shown next to a comment or an activity entry.
///
/// `name` is the current name of an active person; for a person who has left it is the last known
/// name (the stored label) or `None`. It is never an id.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Person {
    pub id: Uuid,
    pub name: Option<String>,
    pub is_bot: bool,
    pub state: PersonState,
}

/// Which people count as active, and whose name may be read.
#[derive(Debug, Clone, Copy)]
pub enum Audience {
    /// Active means a member of the workspace. `hide_left_names` keeps the name of a person who is
    /// not a member from a connected application.
    Workspace { id: Uuid, hide_left_names: bool },
    /// No workspace to judge by (a personal Flow page, or a Flow page with guests): active means an
    /// enabled account.
    Anyone,
}

impl Audience {
    /// The audience of a workspace for the request being served.
    pub fn of_workspace(id: Uuid) -> Self {
        Self::Workspace {
            id,
            hide_left_names: crate::oauth::delegated::current().is_some(),
        }
    }
}

/// Resolves the mentions of a comment.
///
/// `explicit` are ids the client picked; those that are not in `members` are dropped. Typed
/// `@Name` tokens of `body` are added. The result has no duplicates, explicit ones first.
///
/// # Errors
/// More than [`MENTION_CAP`] people.
pub fn resolve(body: &str, explicit: &[Uuid], members: &[Member]) -> Result<Vec<Mention>, ApiError> {
    let by_id: HashMap<Uuid, &Member> = members.iter().map(|member| (member.id, member)).collect();
    let mut found: Vec<Mention> = Vec::new();
    let picked = explicit.iter().filter_map(|id| by_id.get(id).copied());
    for member in picked.chain(typed_names(body, members)) {
        if !found.iter().any(|mention| mention.id == member.id) {
            found.push(Mention {
                id: member.id,
                label: member.name.trim().to_string(),
            });
        }
    }
    if found.len() > MENTION_CAP {
        return Err(ApiError::BadRequest(
            "a comment can mention at most 20 people".to_string(),
        ));
    }
    Ok(found)
}

/// The people named by `@Name` tokens in `body`.
fn typed_names<'a>(body: &str, members: &'a [Member]) -> Vec<&'a Member> {
    if !body.contains('@') {
        return Vec::new();
    }
    // Lower-cased name -> the distinct people who carry it.
    let mut groups: HashMap<Vec<char>, Vec<&Member>> = HashMap::new();
    for member in members {
        let name: Vec<char> = member.name.trim().chars().flat_map(char::to_lowercase).collect();
        if name.is_empty() {
            continue;
        }
        let group = groups.entry(name).or_default();
        if !group.iter().any(|other| other.id == member.id) {
            group.push(member);
        }
    }
    let mut names: Vec<(&Vec<char>, &Vec<&Member>)> = groups.iter().collect();
    names.sort_by(|a, b| b.0.len().cmp(&a.0.len()).then_with(|| a.0.cmp(b.0)));

    let chars: Vec<char> = body.chars().collect();
    let mut found = Vec::new();
    let mut at = 0;
    while at < chars.len() {
        let starts_token = chars.get(at) == Some(&'@')
            && (at == 0 || chars.get(at - 1).is_none_or(|before| !before.is_alphanumeric()));
        if !starts_token {
            at += 1;
            continue;
        }
        let hit = names.iter().find_map(|(name, group)| {
            let used = match_len(&chars, at + 1, name)?;
            let end = at + 1 + used;
            let ascii_word = |c: &char| c.is_ascii_alphanumeric() || *c == '_';
            // A name that ends like a word must not run on into a longer word.
            let runs_on = name.last().is_some_and(ascii_word) && chars.get(end).is_some_and(ascii_word);
            (!runs_on).then_some((used, group))
        });
        match hit {
            Some((used, group)) => {
                // A shared name is consumed but names nobody: the client has to pick.
                if let [only] = group.as_slice() {
                    found.push(*only);
                }
                at += 1 + used;
            }
            None => at += 1,
        }
    }
    found
}

/// How many characters of `chars` from `start` spell `name` (already lower-case), ignoring case.
fn match_len(chars: &[char], start: usize, name: &[char]) -> Option<usize> {
    let mut matched = 0;
    let mut used = 0;
    while matched < name.len() {
        let current = chars.get(start + used)?;
        for lower in current.to_lowercase() {
            if name.get(matched) != Some(&lower) {
                return None;
            }
            matched += 1;
        }
        used += 1;
    }
    Some(used)
}

#[derive(FromQueryResult)]
struct MemberRow {
    id: Uuid,
    name: String,
}

/// Everyone who belongs to a workspace, humans and bots.
///
/// # Errors
/// A database failure.
pub async fn workspace_members<C: ConnectionTrait>(db: &C, workspace_id: Uuid) -> Result<Vec<Member>, ApiError> {
    let rows = MemberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT u.id, COALESCE(u.name, '') AS name \
         FROM workspace_members wm JOIN users u ON u.id = wm.user_id \
         WHERE wm.workspace_id = $1 ORDER BY u.id",
        vec![workspace_id.into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Member {
            id: row.id,
            name: row.name,
        })
        .collect())
}

/// The accounts behind `ids` that exist, by id, for a caller that picks people outside a
/// workspace roster.
///
/// # Errors
/// A database failure.
pub async fn existing_users<C: ConnectionTrait>(db: &C, ids: &[Uuid]) -> Result<Vec<Member>, ApiError> {
    if ids.is_empty() {
        return Ok(Vec::new());
    }
    let rows = MemberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, COALESCE(name, '') AS name FROM users WHERE id = ANY($1) ORDER BY id",
        vec![ids.to_vec().into()],
    ))
    .all(db)
    .await?;
    Ok(rows
        .into_iter()
        .map(|row| Member {
            id: row.id,
            name: row.name,
        })
        .collect())
}

/// The name that goes after `mentioned_by:` in a notification: the actor's name without control
/// characters, at most 80 characters, empty when there is none (the reader words that case).
///
/// # Errors
/// A database failure.
pub async fn actor_display_name<C: ConnectionTrait>(db: &C, actor_id: Uuid) -> Result<String, ApiError> {
    let name = MemberRow::find_by_statement(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT id, COALESCE(name, '') AS name FROM users WHERE id = $1",
        vec![actor_id.into()],
    ))
    .one(db)
    .await?
    .map(|row| row.name)
    .unwrap_or_default();
    Ok(name
        .chars()
        .filter(|ch| !ch.is_control())
        .take(80)
        .collect::<String>()
        .trim()
        .to_string())
}

/// The `content` of a mention notification.
#[must_use]
pub fn notification_content(actor_name: &str) -> String {
    format!("mentioned_by:{actor_name}")
}

/// The stored form of mentions, for the `mentions` column.
#[must_use]
pub fn to_json(mentions: &[Mention]) -> Value {
    json!(
        mentions
            .iter()
            .map(|mention| json!({"id": mention.id, "label": mention.label}))
            .collect::<Vec<_>>()
    )
}

/// Reads a stored `mentions` value; entries that are not `{id, label}` are ignored.
#[must_use]
pub fn from_json(value: &Value) -> Vec<Mention> {
    value
        .as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|item| serde_json::from_value::<Mention>(item.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The ids of `mentions`.
#[must_use]
pub fn ids(mentions: &[Mention]) -> Vec<Uuid> {
    mentions.iter().map(|mention| mention.id).collect()
}

#[derive(FromQueryResult)]
struct PersonRow {
    id: Uuid,
    name: Option<String>,
    entity_type: String,
    is_active: bool,
    member: bool,
}

/// Shows people. `hints` are the stored labels, used for a person who has left.
///
/// An id that has no account at all still gets an entry (`state: removed`, the hint as name), so
/// every id asked for is in the answer.
///
/// # Errors
/// A database failure.
pub async fn people<C: ConnectionTrait>(
    db: &C,
    audience: Audience,
    ids: &[Uuid],
    hints: &HashMap<Uuid, String>,
) -> Result<HashMap<Uuid, Person>, ApiError> {
    let wanted: Vec<Uuid> = ids.iter().copied().collect::<HashSet<_>>().into_iter().collect();
    let mut shown: HashMap<Uuid, Person> = HashMap::new();
    if !wanted.is_empty() {
        let workspace = match audience {
            Audience::Workspace { id, .. } => Some(id),
            Audience::Anyone => None,
        };
        let rows = PersonRow::find_by_statement(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT u.id, u.name, u.entity_type, u.is_active, (m.user_id IS NOT NULL) AS member \
             FROM users u \
             LEFT JOIN workspace_members m ON m.workspace_id = $2 AND m.user_id = u.id \
             WHERE u.id = ANY($1)",
            vec![wanted.clone().into(), workspace.into()],
        ))
        .all(db)
        .await?;
        for row in rows {
            let active = match audience {
                Audience::Workspace { .. } => row.member,
                Audience::Anyone => row.is_active,
            };
            let may_read_name = active
                || !matches!(
                    audience,
                    Audience::Workspace {
                        hide_left_names: true,
                        ..
                    }
                );
            let current = row.name.filter(|name| !name.trim().is_empty());
            let readable = if may_read_name { current } else { None };
            let name = readable.or_else(|| non_blank(hints.get(&row.id)));
            shown.insert(
                row.id,
                Person {
                    id: row.id,
                    name,
                    is_bot: row.entity_type == "bot",
                    state: if active {
                        PersonState::Active
                    } else {
                        PersonState::Removed
                    },
                },
            );
        }
    }
    for id in wanted {
        shown.entry(id).or_insert_with(|| Person {
            id,
            name: non_blank(hints.get(&id)),
            is_bot: false,
            state: PersonState::Removed,
        });
    }
    Ok(shown)
}

fn non_blank(label: Option<&String>) -> Option<String> {
    label.filter(|label| !label.trim().is_empty()).cloned()
}

/// Shows the stored `mentions` with the current state of each person, in stored order.
pub fn mention_people(mentions: &[Mention], shown: &HashMap<Uuid, Person>) -> Vec<Person> {
    mentions
        .iter()
        .map(|mention| {
            shown.get(&mention.id).cloned().unwrap_or_else(|| Person {
                id: mention.id,
                name: non_blank(Some(&mention.label)),
                is_bot: false,
                state: PersonState::Removed,
            })
        })
        .collect()
}

/// Labels of stored mentions by id, the hints [`people`] wants.
#[must_use]
pub fn hints<'a>(stored: impl IntoIterator<Item = &'a Mention>) -> HashMap<Uuid, String> {
    stored
        .into_iter()
        .map(|mention| (mention.id, mention.label.clone()))
        .collect()
}

/// `{id, name}` pairs for a webhook: the current name, else the stored label (`hints`), else
/// null; never an id.
///
/// # Errors
/// A database failure.
pub async fn webhook_pairs<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    ids: &[Uuid],
    hints: &HashMap<Uuid, String>,
) -> Result<Vec<Value>, ApiError> {
    let shown = people(
        db,
        Audience::Workspace {
            id: workspace_id,
            hide_left_names: false,
        },
        ids,
        hints,
    )
    .await?;
    Ok(ids
        .iter()
        .filter_map(|id| shown.get(id))
        .map(|person| json!({"id": person.id, "name": person.name}))
        .collect())
}

/// Resolves the mentions of a work item or proposal comment against the people of a workspace.
/// The roster is read only when there is something to resolve.
///
/// # Errors
/// More than [`MENTION_CAP`] people, or a database failure.
pub async fn resolve_in_workspace<C: ConnectionTrait>(
    db: &C,
    workspace_id: Uuid,
    body: &str,
    explicit: &[Uuid],
) -> Result<Vec<Mention>, ApiError> {
    if explicit.is_empty() && !body.contains('@') {
        return Ok(Vec::new());
    }
    let members = workspace_members(db, workspace_id).await?;
    resolve(body, explicit, &members)
}

/// Everything about the people a list of comments shows: authors and mentioned people.
///
/// # Errors
/// A database failure.
pub async fn people_of_comments<C: ConnectionTrait>(
    db: &C,
    audience: Audience,
    authors: impl IntoIterator<Item = Uuid>,
    stored: &[&[Mention]],
) -> Result<HashMap<Uuid, Person>, ApiError> {
    let mut wanted: Vec<Uuid> = authors.into_iter().collect();
    wanted.extend(stored.iter().flat_map(|list| list.iter().map(|mention| mention.id)));
    let labels = hints(stored.iter().flat_map(|list| list.iter()));
    people(db, audience, &wanted, &labels).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn member(name: &str) -> Member {
        Member {
            id: Uuid::new_v4(),
            name: name.to_string(),
        }
    }

    fn labels(found: &[Mention]) -> Vec<&str> {
        found.iter().map(|mention| mention.label.as_str()).collect()
    }

    #[test]
    fn a_typed_name_is_a_mention_whatever_its_case() {
        let members = [member("Ada Lovelace"), member("bob")];
        let found = resolve("hi @ada lovelace and @BOB!", &[], &members).expect("resolves");
        assert_eq!(labels(&found), ["Ada Lovelace", "bob"]);
    }

    #[test]
    fn the_longest_name_wins() {
        let members = [member("Ann"), member("Ann Lee")];
        let found = resolve("@Ann Lee please, and @Ann", &[], &members).expect("resolves");
        assert_eq!(labels(&found), ["Ann Lee", "Ann"]);
    }

    #[test]
    fn a_shared_name_is_not_resolved() {
        let members = [member("Sam"), member("sam"), member("Ann")];
        let found = resolve("@Sam and @Ann", &[], &members).expect("resolves");
        assert_eq!(labels(&found), ["Ann"]);
    }

    #[test]
    fn an_explicit_pick_settles_a_shared_name() {
        let members = [member("Sam"), member("Sam")];
        let picked = members[1].id;
        let found = resolve("@Sam", &[picked], &members).expect("resolves");
        assert_eq!(ids(&found), vec![picked]);
    }

    #[test]
    fn an_address_or_a_longer_word_is_not_a_mention() {
        let members = [member("bob"), member("Ann")];
        let found = resolve("mail a@bob.com or @bobby or @Annex", &[], &members).expect("resolves");
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn explicit_ids_outside_the_roster_are_dropped_and_duplicates_collapse() {
        let members = [member("bob")];
        let found = resolve("@bob", &[members[0].id, members[0].id, Uuid::new_v4()], &members).expect("resolves");
        assert_eq!(ids(&found), vec![members[0].id]);
    }

    #[test]
    fn a_name_in_chinese_needs_no_space_after_it() {
        let members = [member("张三")];
        let found = resolve("@张三请看", &[], &members).expect("resolves");
        assert_eq!(labels(&found), ["张三"]);
    }

    #[test]
    fn more_than_twenty_people_are_refused() {
        let members: Vec<Member> = (0..21).map(|n| member(&format!("user{n:02}x"))).collect();
        let body = members.iter().fold(String::new(), |body, m| body + "@" + &m.name + " ");
        assert!(resolve(&body, &[], &members).is_err());
    }

    #[test]
    fn stored_mentions_round_trip() {
        let stored = vec![Mention {
            id: Uuid::new_v4(),
            label: "Ada".to_string(),
        }];
        assert_eq!(from_json(&to_json(&stored)), stored);
        assert!(from_json(&json!(["not-an-object", 3])).is_empty());
    }
}
