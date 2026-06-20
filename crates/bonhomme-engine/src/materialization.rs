use crate::{
    CacheStatus, MaterializedBranch, Storage,
    rows::{GraphCacheRow, operation_fingerprint},
};
use anyhow::{Result, bail};
use bonhomme_core::{RenderedFile, SemanticGraph, materialize};
use uuid::Uuid;

impl Storage {
    pub async fn materialize_branch(
        &self,
        repository_name: &str,
        branch_name: &str,
    ) -> Result<MaterializedBranch> {
        let repository = self.repository_by_name(repository_name).await?;
        let branch = self.branch_by_name(repository.id, branch_name).await?;
        let operations = self.collect_branch_operations(branch.id, None).await?;
        let operation_count = operations.len() as i64;
        let operation_fingerprint = operation_fingerprint(&operations);

        if let Some((graph, files)) = self
            .cached_materialization(branch.id, operation_count, &operation_fingerprint)
            .await?
        {
            return Ok(MaterializedBranch {
                repository,
                branch,
                operations,
                graph,
                files,
                cache_status: CacheStatus::Hit,
            });
        }

        let graph = materialize(&operations)?;
        let files = self.plugin.render(&graph);
        self.store_graph_cache(
            repository.id,
            branch.id,
            operation_count,
            &operation_fingerprint,
            &graph,
            &files,
        )
        .await?;
        Ok(MaterializedBranch {
            repository,
            branch,
            operations,
            graph,
            files,
            cache_status: CacheStatus::Miss,
        })
    }

    pub async fn materialize_branch_at_position(
        &self,
        branch_id: Uuid,
        operation_count: i64,
    ) -> Result<MaterializedBranch> {
        if operation_count < 0 {
            bail!("operation position must be non-negative");
        }
        let branch = self.branch_by_id(branch_id).await?;
        let repository = self.repository_by_id(branch.repository_id).await?;
        let operations = self
            .collect_branch_operations(branch.id, Some(operation_count))
            .await?;
        if operations.len() as i64 != operation_count {
            bail!(
                "branch {} has {} visible operations, not {}",
                branch.name,
                operations.len(),
                operation_count
            );
        }
        let graph = materialize(&operations)?;
        let files = self.plugin.render(&graph);
        Ok(MaterializedBranch {
            repository,
            branch,
            operations,
            graph,
            files,
            cache_status: CacheStatus::Miss,
        })
    }

    async fn cached_materialization(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        operation_fingerprint: &str,
    ) -> Result<Option<(SemanticGraph, Vec<RenderedFile>)>> {
        let Some(row) = sqlx::query_as::<_, GraphCacheRow>(
            r#"
            SELECT graph, rendered_files
            FROM graph_cache
            WHERE branch_id = $1 AND operation_count = $2 AND operation_fingerprint = $3
            "#,
        )
        .bind(branch_id)
        .bind(operation_count)
        .bind(operation_fingerprint)
        .fetch_optional(&self.pool)
        .await?
        else {
            return Ok(None);
        };

        let graph = serde_json::from_value(row.graph)?;
        let files = serde_json::from_value(row.rendered_files)?;
        Ok(Some((graph, files)))
    }

    async fn store_graph_cache(
        &self,
        repository_id: Uuid,
        branch_id: Uuid,
        operation_count: i64,
        operation_fingerprint: &str,
        graph: &SemanticGraph,
        files: &[RenderedFile],
    ) -> Result<()> {
        sqlx::query(
            r#"
            INSERT INTO graph_cache (branch_id, repository_id, operation_count, operation_fingerprint, graph, rendered_files)
            VALUES ($1, $2, $3, $4, $5, $6)
            ON CONFLICT (branch_id) DO UPDATE
            SET operation_count = EXCLUDED.operation_count,
                operation_fingerprint = EXCLUDED.operation_fingerprint,
                graph = EXCLUDED.graph,
                rendered_files = EXCLUDED.rendered_files,
                updated_at = now()
            "#,
        )
        .bind(branch_id)
        .bind(repository_id)
        .bind(operation_count)
        .bind(operation_fingerprint)
        .bind(serde_json::to_value(graph)?)
        .bind(serde_json::to_value(files)?)
        .execute(&self.pool)
        .await?;
        Ok(())
    }
}
