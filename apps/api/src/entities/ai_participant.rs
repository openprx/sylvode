use platform::timestamp::Timestamp;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "ai_participants")]
pub struct Model {
    /// The bot's user id as text. A bot may be a participant of several projects, so the key is the
    /// project and the id together.
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: String,
    #[sea_orm(primary_key, auto_increment = false)]
    pub project_id: Uuid,
    pub name: String,
    pub domain_overrides: Option<Json>,
    pub max_domain_level: String,
    pub can_veto_human_consensus: bool,
    pub reason_min_length: i32,
    pub is_active: bool,
    pub registered_by: Uuid,
    #[serde(default, with = "platform::timestamp::with_offset::option")]
    pub last_active_at: Option<Timestamp>,
    #[serde(with = "platform::timestamp::with_offset")]
    pub created_at: Timestamp,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
    #[sea_orm(
        belongs_to = "super::user::Entity",
        from = "Column::RegisteredBy",
        to = "super::user::Column::Id"
    )]
    RegisteredByUser,
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl Related<super::user::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::RegisteredByUser.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

/// The entity matches the table: a row written through it reads back, the same bot can be a
/// participant of two projects, and the same bot twice in one project is refused.
#[cfg(test)]
#[allow(clippy::expect_used, clippy::panic)]
mod tests {
    use super::{ActiveModel, Entity};
    use crate::routes::context::tenant_fixture::{exec, seed_tenant};
    use crate::scratch_or_skip;
    use sea_orm::{ActiveModelTrait, EntityTrait, Set};
    use uuid::Uuid;

    #[tokio::test]
    async fn the_entity_reads_and_writes_the_current_table() {
        let scratch = scratch_or_skip!("entity_ai_participant");
        let db = &scratch.db;
        let tenant = seed_tenant(db, "ap").await;
        let second = Uuid::new_v4();
        exec(
            db,
            "INSERT INTO projects (id, workspace_id, key, name, created_by) VALUES ($1, $2, 'APX', 'second', $3)",
            vec![second.into(), tenant.workspace_id.into(), tenant.member_id.into()],
        )
        .await;
        let bot = tenant.member_id.to_string();
        let row = |project: Uuid| ActiveModel {
            id: Set(bot.clone()),
            project_id: Set(project),
            name: Set("participant".to_string()),
            domain_overrides: Set(None),
            max_domain_level: Set("observer".to_string()),
            can_veto_human_consensus: Set(false),
            reason_min_length: Set(0),
            is_active: Set(true),
            registered_by: Set(tenant.member_id),
            last_active_at: Set(None),
            created_at: Set(platform::timestamp::Timestamp::now()),
        };
        row(tenant.project_id).insert(db).await.expect("first project");
        row(second).insert(db).await.expect("the same bot in a second project");
        assert!(row(second).insert(db).await.is_err(), "once per project");
        let found = Entity::find_by_id((bot.clone(), second)).one(db).await.expect("query");
        assert_eq!(found.map(|row| row.project_id), Some(second));
        assert_eq!(Entity::find().all(db).await.expect("read").len(), 2);
        scratch.drop_self().await;
    }
}
