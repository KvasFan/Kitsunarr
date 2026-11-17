//! Custom filters (Sonarr's `CustomFilter`): saved filters of the web interface's lists.

use kitsunarr_domain::api::settings::CustomFilter;
use sqlx::{FromRow, SqliteExecutor};

use crate::{DbError, DbResult};

#[derive(FromRow)]
struct Row {
    id: i64,
    r#type: String,
    label: String,
    filters: String,
}

impl Row {
    fn into_filter(self) -> CustomFilter {
        CustomFilter {
            id: self.id,
            r#type: self.r#type,
            label: self.label,
            filters: serde_json::from_str(&self.filters).unwrap_or_default(),
        }
    }
}

fn checked(f: &CustomFilter) -> DbResult<String> {
    if f.r#type.trim().is_empty() {
        return Err(DbError::Invalid("A custom filter needs a type.".into()));
    }
    if f.label.trim().is_empty() {
        return Err(DbError::Invalid("A custom filter needs a label.".into()));
    }
    serde_json::to_string(&f.filters).map_err(|e| DbError::Invalid(e.to_string()))
}

pub async fn list(ex: impl SqliteExecutor<'_>) -> DbResult<Vec<CustomFilter>> {
    let rows: Vec<Row> =
        sqlx::query_as("SELECT id, type, label, filters FROM custom_filters ORDER BY id")
            .fetch_all(ex)
            .await?;
    Ok(rows.into_iter().map(Row::into_filter).collect())
}

pub async fn get(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<CustomFilter> {
    let row: Option<Row> =
        sqlx::query_as("SELECT id, type, label, filters FROM custom_filters WHERE id = ?")
            .bind(id)
            .fetch_optional(ex)
            .await?;
    row.map(Row::into_filter).ok_or(DbError::NotFound)
}

pub async fn create(ex: impl SqliteExecutor<'_>, f: &CustomFilter) -> DbResult<i64> {
    let filters = checked(f)?;
    sqlx::query_scalar(
        "INSERT INTO custom_filters (type, label, filters) VALUES (?, ?, ?) RETURNING id",
    )
    .bind(f.r#type.trim())
    .bind(f.label.trim())
    .bind(filters)
    .fetch_one(ex)
    .await
    .map_err(|e| DbError::from_write(e, "custom filter"))
}

pub async fn update(ex: impl SqliteExecutor<'_>, f: &CustomFilter) -> DbResult<()> {
    let filters = checked(f)?;
    let r = sqlx::query("UPDATE custom_filters SET type = ?, label = ?, filters = ? WHERE id = ?")
        .bind(f.r#type.trim())
        .bind(f.label.trim())
        .bind(filters)
        .bind(f.id)
        .execute(ex)
        .await
        .map_err(|e| DbError::from_write(e, "custom filter"))?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn delete(ex: impl SqliteExecutor<'_>, id: i64) -> DbResult<()> {
    let r = sqlx::query("DELETE FROM custom_filters WHERE id = ?")
        .bind(id)
        .execute(ex)
        .await?;
    if r.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn custom_filters_round_trip() {
        let db = crate::Database::open_in_memory().await.unwrap();
        let mut f = CustomFilter {
            id: 0,
            r#type: "series".into(),
            label: "Anime".into(),
            filters: vec![json!({"key": "seriesType", "value": ["anime"], "type": "equal"})],
        };
        f.id = create(db.pool(), &f).await.unwrap();
        assert_eq!(get(db.pool(), f.id).await.unwrap(), f);
        f.label = "Anime shows".into();
        update(db.pool(), &f).await.unwrap();
        assert_eq!(list(db.pool()).await.unwrap(), vec![f.clone()]);
        f.label = " ".into();
        assert!(matches!(
            update(db.pool(), &f).await,
            Err(DbError::Invalid(_))
        ));
        delete(db.pool(), f.id).await.unwrap();
        assert!(matches!(get(db.pool(), f.id).await, Err(DbError::NotFound)));
    }
}
