use platform::timestamp::Timestamp;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "workspaces")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub slug: String,
    pub name: String,
    pub created_by: Option<Uuid>,
    #[serde(with = "platform::timestamp::with_offset")]
    pub created_at: Timestamp,
    #[serde(with = "platform::timestamp::with_offset")]
    pub updated_at: Timestamp,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(has_many = "super::workspace_member::Entity")]
    WorkspaceMembers,
    #[sea_orm(has_many = "super::project::Entity")]
    Projects,
}

impl Related<super::workspace_member::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::WorkspaceMembers.def()
    }
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Projects.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}
