use crate::{
    CacheStatus, MaterializedBranch, MaterializedGraph, Storage, rows::operation_fingerprint,
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

    pub async fn materialize_branch_graph(
        &self,
        repository_name: &str,
        branch_name: &str,
    ) -> Result<MaterializedGraph> {
        let repository = self.repository_by_name(repository_name).await?;
        let branch = self.branch_by_name(repository.id, branch_name).await?;
        let operations = self.collect_branch_operations(branch.id, None).await?;
        let operation_count = operations.len() as i64;
        let operation_fingerprint = operation_fingerprint(&operations);

        if let Some(graph) = self
            .cached_graph_materialization(branch.id, operation_count, &operation_fingerprint)
            .await?
        {
            return Ok(MaterializedGraph {
                repository,
                branch,
                operations,
                graph,
                cache_status: CacheStatus::Hit,
            });
        }

        let graph = materialize(&operations)?;
        Ok(MaterializedGraph {
            repository,
            branch,
            operations,
            graph,
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

    pub async fn materialize_branch_graph_at_position(
        &self,
        branch_id: Uuid,
        operation_count: i64,
    ) -> Result<MaterializedGraph> {
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
        Ok(MaterializedGraph {
            repository,
            branch,
            operations,
            graph,
            cache_status: CacheStatus::Miss,
        })
    }

    async fn cached_materialization(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        operation_fingerprint: &str,
    ) -> Result<Option<(SemanticGraph, Vec<RenderedFile>)>> {
        let Some((graph, files)) = self
            .backend
            .get_graph_cache(branch_id, operation_count, operation_fingerprint)
            .await?
        else {
            return Ok(None);
        };

        let graph = serde_json::from_value(graph)?;
        let files = serde_json::from_value(files)?;
        Ok(Some((graph, files)))
    }

    async fn cached_graph_materialization(
        &self,
        branch_id: Uuid,
        operation_count: i64,
        operation_fingerprint: &str,
    ) -> Result<Option<SemanticGraph>> {
        let Some(graph) = self
            .backend
            .get_graph_cache_graph(branch_id, operation_count, operation_fingerprint)
            .await?
        else {
            return Ok(None);
        };

        Ok(Some(serde_json::from_value(graph)?))
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
        self.backend
            .store_graph_cache(
                repository_id,
                branch_id,
                operation_count,
                operation_fingerprint,
                serde_json::to_value(graph)?,
                serde_json::to_value(files)?,
            )
            .await
    }
}
