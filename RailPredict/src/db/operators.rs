//! Read access to the operators reference table.

use crate::db::Db;

/// An operator (TOC) reference row: code, friendly name, brand colour.
#[derive(Debug, Clone, sqlx::FromRow, PartialEq, Eq)]
pub struct Operator {
    pub toc: String,
    pub name: String,
    pub brand_color: String,
}

/// List all operators, ordered by friendly name.
pub async fn list_operators(db: &Db) -> sqlx::Result<Vec<Operator>> {
    sqlx::query_as::<_, Operator>(
        "SELECT toc, name, brand_color FROM operators ORDER BY name",
    )
    .fetch_all(db)
    .await
}
