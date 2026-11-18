use platform::timestamp::Timestamp;
use sea_orm::entity::prelude::*;
use serde::{Deserialize, Serialize};
use time::Date;

#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel, Serialize, Deserialize)]
#[sea_orm(table_name = "sprints")]
pub struct Model {
    #[sea_orm(primary_key, auto_increment = false)]
    pub id: Uuid,
    pub project_id: Uuid,
    pub name: String,
    pub description: String,
    #[serde(default, with = "super::iso_date::option")]
    pub start_date: Option<Date>,
    #[serde(default, with = "super::iso_date::option")]
    pub end_date: Option<Date>,
    pub status: String,
    #[serde(with = "platform::timestamp::with_offset")]
    pub created_at: Timestamp,
    #[serde(with = "platform::timestamp::with_offset")]
    pub updated_at: Timestamp,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {
    #[sea_orm(
        belongs_to = "super::project::Entity",
        from = "Column::ProjectId",
        to = "super::project::Column::Id"
    )]
    Project,
}

impl Related<super::project::Entity> for Entity {
    fn to() -> RelationDef {
        Relation::Project.def()
    }
}

impl ActiveModelBehavior for ActiveModel {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn calendar_dates_and_timezone_offsets_keep_their_wire_format() {
        let input = serde_json::json!({
            "id": Uuid::nil(),
            "project_id": Uuid::nil(),
            "name": "Sprint",
            "description": "",
            "start_date": "2026-11-18",
            "status": "planning",
            "created_at": "2026-11-18T04:00:00+04:00",
            "updated_at": "2026-11-18T00:00:00Z"
        });
        let model: Model = serde_json::from_value(input.clone()).expect("date and offset strings decode");
        assert_eq!(model.start_date, Some(time::macros::date!(2026 - 11 - 18)));
        assert_eq!(model.end_date, None);
        let mut expected = input;
        assert!(
            expected
                .as_object_mut()
                .expect("fixture object")
                .insert("end_date".into(), serde_json::Value::Null)
                .is_none()
        );
        assert_eq!(serde_json::to_value(model).expect("sprint serializes"), expected);
    }
}
