use anyhow::Result;
use axum::{
    Router,
    extract::{Query, State},
    http::StatusCode,
    response::{Html, IntoResponse, Response},
    routing::get,
};
use bonhomme_core::{
    Operation, OperationRecord, ReferenceNode, SemanticGraph, SymbolNode, metadata_string,
};
use bonhomme_engine::{MaterializedGraph, Storage};
use serde::Deserialize;
use serde_json::json;
use std::{
    collections::{BTreeMap, btree_map::Entry},
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, RwLock},
};
use tokio::{fs, net::TcpListener};
use tracing::info;
use uuid::Uuid;

const MAX_BODY_BYTES: usize = 160_000;
const MAX_SOURCE_BYTES: usize = 240_000;

#[derive(Clone)]
pub struct ExplorerContext {
    storage: Storage,
    root: PathBuf,
    repository_name: String,
    default_branch: String,
    config_label: String,
    database_label: String,
    snapshots: Arc<RwLock<BTreeMap<SnapshotKey, Arc<ExplorerSnapshotData>>>>,
}

#[derive(Debug, Deserialize)]
struct ExplorerQuery {
    branch: Option<String>,
    as_of: Option<i64>,
    symbol: Option<Uuid>,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
struct SnapshotKey {
    branch_name: String,
    as_of: Option<i64>,
}

#[allow(clippy::too_many_arguments)]
pub async fn serve(
    storage: Storage,
    root: PathBuf,
    repository_name: String,
    default_branch: String,
    config_label: String,
    database_label: String,
    addr: SocketAddr,
    open: bool,
) -> Result<()> {
    let repository = storage.repository_by_name(&repository_name).await?;
    storage
        .branch_by_name(repository.id, &default_branch)
        .await?;

    let listener = TcpListener::bind(addr).await?;
    let local_addr = listener.local_addr()?;
    let url = explorer_url(local_addr);
    let context = ExplorerContext {
        storage,
        root,
        repository_name,
        default_branch,
        config_label,
        database_label,
        snapshots: Arc::new(RwLock::new(BTreeMap::new())),
    };
    let context_config = context.config_label.clone();
    let context_storage = context.database_label.clone();

    write_status_file(&context, &url).await?;

    let app = Router::new()
        .route("/", get(index))
        .route("/symbol", get(symbol_fragment))
        .route("/health", get(health))
        .with_state(context);

    println!("bonhomme explorer listening on {url}");
    println!("repo-scoped config: {context_config}");
    println!("storage: {context_storage}");
    if open {
        open_browser(&url);
    }

    info!("bonhomme explorer listening on {url}");
    axum::serve(listener, app).await?;
    Ok(())
}

async fn health() -> &'static str {
    "ok"
}

async fn index(
    State(context): State<ExplorerContext>,
    Query(query): Query<ExplorerQuery>,
) -> ExplorerResult {
    let snapshot = ExplorerSnapshot::load(&context, query).await?;
    Ok(Html(render_page(&context, &snapshot)))
}

async fn symbol_fragment(
    State(context): State<ExplorerContext>,
    Query(query): Query<ExplorerQuery>,
) -> ExplorerResult {
    let snapshot = ExplorerSnapshot::load(&context, query).await?;
    Ok(Html(render_selection_panel(&context, &snapshot)))
}

struct ExplorerSnapshotData {
    branch_name: String,
    as_of: Option<i64>,
    materialized: MaterializedGraph,
    latest_operation_count: i64,
    branches: Vec<bonhomme_core::Branch>,
    branch_names: BTreeMap<Uuid, String>,
    operation_records: Vec<OperationRecord>,
}

struct ExplorerSnapshot {
    data: Arc<ExplorerSnapshotData>,
    selected_symbol_id: Option<Uuid>,
}

impl std::ops::Deref for ExplorerSnapshot {
    type Target = ExplorerSnapshotData;

    fn deref(&self) -> &Self::Target {
        &self.data
    }
}

impl ExplorerSnapshot {
    async fn load(context: &ExplorerContext, query: ExplorerQuery) -> Result<Self> {
        let key = SnapshotKey {
            branch_name: query
                .branch
                .unwrap_or_else(|| context.default_branch.clone()),
            as_of: query.as_of,
        };
        let data = context.snapshot_data(key).await?;
        let selected_symbol_id = query
            .symbol
            .filter(|id| data.materialized.graph.symbols.contains_key(id))
            .or_else(|| {
                data.materialized
                    .graph
                    .root_symbols()
                    .first()
                    .map(|symbol| symbol.id)
            });

        Ok(Self {
            data,
            selected_symbol_id,
        })
    }

    fn selected_symbol(&self) -> Option<&SymbolNode> {
        self.selected_symbol_id
            .and_then(|id| self.materialized.graph.symbols.get(&id))
    }
}

impl ExplorerContext {
    async fn snapshot_data(&self, key: SnapshotKey) -> Result<Arc<ExplorerSnapshotData>> {
        if let Some(snapshot) = self.cached_snapshot(&key)? {
            return Ok(snapshot);
        }

        let loaded = Arc::new(ExplorerSnapshotData::load(self, &key).await?);
        let mut snapshots = self
            .snapshots
            .write()
            .map_err(|_| anyhow::anyhow!("explorer snapshot cache lock poisoned"))?;
        Ok(match snapshots.entry(key) {
            Entry::Occupied(entry) => Arc::clone(entry.get()),
            Entry::Vacant(entry) => Arc::clone(entry.insert(loaded)),
        })
    }

    fn cached_snapshot(&self, key: &SnapshotKey) -> Result<Option<Arc<ExplorerSnapshotData>>> {
        let snapshots = self
            .snapshots
            .read()
            .map_err(|_| anyhow::anyhow!("explorer snapshot cache lock poisoned"))?;
        Ok(snapshots.get(key).cloned())
    }
}

impl ExplorerSnapshotData {
    async fn load(context: &ExplorerContext, key: &SnapshotKey) -> Result<Self> {
        let repository = context
            .storage
            .repository_by_name(&context.repository_name)
            .await?;
        let branches = context.storage.list_branches(repository.id).await?;
        let branch = context
            .storage
            .branch_by_name(repository.id, &key.branch_name)
            .await?;
        let materialized = if let Some(as_of) = key.as_of {
            context
                .storage
                .materialize_branch_graph_at_position(branch.id, as_of)
                .await?
        } else {
            context
                .storage
                .materialize_branch_graph(&context.repository_name, &key.branch_name)
                .await?
        };
        let latest_operation_count = if key.as_of.is_some() {
            context
                .storage
                .collect_branch_operations(branch.id, None)
                .await?
                .len() as i64
        } else {
            materialized.operations.len() as i64
        };
        let operation_records = context.storage.list_operations(repository.id).await?;
        let branch_names = branches
            .iter()
            .map(|branch| (branch.id, branch.name.clone()))
            .collect::<BTreeMap<_, _>>();

        Ok(Self {
            branch_name: key.branch_name.clone(),
            as_of: key.as_of,
            materialized,
            latest_operation_count,
            branches,
            branch_names,
            operation_records,
        })
    }
}

type ExplorerResult = std::result::Result<Html<String>, ExplorerError>;

struct ExplorerError(anyhow::Error);

impl From<anyhow::Error> for ExplorerError {
    fn from(error: anyhow::Error) -> Self {
        Self(error)
    }
}

impl IntoResponse for ExplorerError {
    fn into_response(self) -> Response {
        let message = format!("{:#}", self.0);
        let status = if message.contains("does not exist") || message.contains("not found") {
            StatusCode::NOT_FOUND
        } else if message.contains("not supported") || message.contains("must be") {
            StatusCode::BAD_REQUEST
        } else {
            StatusCode::INTERNAL_SERVER_ERROR
        };
        let body = format!(
            "{}{}{}",
            "<!doctype html><html><head><meta charset=\"utf-8\"><title>bonhomme explorer error</title>",
            STYLES,
            &format!(
                "</head><body><main class=\"error-page\"><h1>Explorer error</h1><p>{}</p></main></body></html>",
                escape_html(&message)
            )
        );
        (status, Html(body)).into_response()
    }
}

fn render_page(context: &ExplorerContext, snapshot: &ExplorerSnapshot) -> String {
    let title = format!(
        "bonhomme explorer · {} · {}",
        context.repository_name, snapshot.branch_name
    );
    let as_of_label = snapshot
        .as_of
        .map(|position| format!("op {position}"))
        .unwrap_or_else(|| "latest".to_string());

    format!(
        r##"<!doctype html>
<html lang="en">
<head>
  <meta charset="utf-8">
  <meta name="viewport" content="width=device-width, initial-scale=1">
  <link rel="icon" href="data:,">
  <title>{title}</title>
  {styles}
  {scripts}
</head>
<body hx-target="#selection-panel" hx-swap="outerHTML">
  <header class="topbar">
    <div class="title-block">
      <div class="eyebrow">bonhomme explorer</div>
      <h1>{repo}</h1>
    </div>
    <div class="topbar-tools">
      <form class="top-controls" method="get" action="/">
        <label class="field"><span class="visually-hidden">Branch</span>{branch_select}</label>
        <label class="field"><span class="visually-hidden">As of operation</span><input type="number" min="0" max="{latest_count}" name="as_of" value="{as_of_value}" placeholder="latest"></label>
        {symbol_input}
        <button type="submit">View</button>
        <a class="button secondary" href="{latest_href}">Latest</a>
      </form>
      <div class="meta">
        <span>{branch}</span>
        <span>{as_of}</span>
        <span>{symbols} symbols</span>
        <span>{refs} refs</span>
      </div>
    </div>
  </header>
  <main class="explorer-layout">
    <aside class="symbol-rail">
      <div class="section-title">Symbols</div>
      {tree}
    </aside>
    <article class="reading-column">
      {selection_panel}
      <details class="environment">
        <summary>Environment</summary>
        <dl>
          <div><dt>root</dt><dd>{root}</dd></div>
          <div><dt>config</dt><dd>{config}</dd></div>
          <div><dt>storage</dt><dd>{database}</dd></div>
        </dl>
      </details>
    </article>
  </main>
</body>
</html>"##,
        title = escape_html(&title),
        styles = STYLES,
        scripts = SCRIPTS,
        repo = escape_html(&context.repository_name),
        branch = escape_html(&snapshot.branch_name),
        as_of = escape_html(&as_of_label),
        symbols = snapshot.materialized.graph.symbols.len(),
        refs = snapshot.materialized.graph.references.len(),
        branch_select = render_branch_select(snapshot),
        latest_count = snapshot.latest_operation_count,
        as_of_value = snapshot
            .as_of
            .map(|value| value.to_string())
            .unwrap_or_default(),
        symbol_input = snapshot
            .selected_symbol_id
            .map(|id| format!(r#"<input type="hidden" name="symbol" value="{id}">"#))
            .unwrap_or_default(),
        latest_href = link_to(&snapshot.branch_name, None, snapshot.selected_symbol_id),
        root = escape_html(&context.root.display().to_string()),
        config = escape_html(&context.config_label),
        database = escape_html(&context.database_label),
        tree = render_tree(snapshot),
        selection_panel = render_selection_panel(context, snapshot),
    )
}

fn render_selection_panel(context: &ExplorerContext, snapshot: &ExplorerSnapshot) -> String {
    let selected = snapshot.selected_symbol();
    format!(
        r#"<div id="selection-panel" class="selection-panel">
  <section class="content-section identity-section">
    {detail}
  </section>
  <section class="content-section source-section">
    <div class="section-title">Rendered Source</div>
    {files}
  </section>
  <section class="content-grid">
    <div class="content-section">
      {inspector}
    </div>
    <div class="content-section">
      <div class="section-title">Visible Operations</div>
      {operations}
    </div>
  </section>
</div>"#,
        detail = render_symbol_detail(snapshot, selected),
        files = render_files(context, snapshot, selected),
        inspector = render_inspector(snapshot, selected),
        operations = render_operations(snapshot),
    )
}

fn render_branch_select(snapshot: &ExplorerSnapshot) -> String {
    let mut options = String::new();
    for branch in &snapshot.branches {
        let selected = if branch.name == snapshot.branch_name {
            " selected"
        } else {
            ""
        };
        options.push_str(&format!(
            r#"<option value="{}"{}>{}</option>"#,
            escape_attr(&branch.name),
            selected,
            escape_html(&branch.name)
        ));
    }
    format!(r#"<select name="branch">{options}</select>"#)
}

fn render_tree(snapshot: &ExplorerSnapshot) -> String {
    let mut children_by_parent = BTreeMap::<Option<Uuid>, Vec<&SymbolNode>>::new();
    for symbol in snapshot.materialized.graph.symbols.values() {
        children_by_parent
            .entry(symbol.parent_id)
            .or_default()
            .push(symbol);
    }
    for children in children_by_parent.values_mut() {
        sort_symbol_refs(children);
    }
    let Some(roots) = children_by_parent.get(&None) else {
        return "<p class=\"muted\">No symbols on this branch yet.</p>".to_string();
    };
    let mut out = String::from("<ul class=\"symbol-tree\">");
    for symbol in roots {
        render_tree_node(snapshot, symbol, &children_by_parent, &mut out);
    }
    out.push_str("</ul>");
    out
}

fn render_tree_node(
    snapshot: &ExplorerSnapshot,
    symbol: &SymbolNode,
    children_by_parent: &BTreeMap<Option<Uuid>, Vec<&SymbolNode>>,
    out: &mut String,
) {
    let selected = snapshot.selected_symbol_id == Some(symbol.id);
    let link_attrs = symbol_link_attrs(snapshot, symbol.id);
    let aria_current = if selected {
        r#" aria-current="true""#
    } else {
        ""
    };
    out.push_str(&format!(
        r#"<li><a class="tree-link{}"{} {}><span class="kind">{}</span><span>{}</span></a>"#,
        if selected { " selected" } else { "" },
        aria_current,
        link_attrs,
        escape_html(&symbol.kind),
        escape_html(&symbol.name)
    ));
    if let Some(children) = children_by_parent.get(&Some(symbol.id))
        && !children.is_empty()
    {
        out.push_str("<ul>");
        for child in children {
            render_tree_node(snapshot, child, children_by_parent, out);
        }
        out.push_str("</ul>");
    }
    out.push_str("</li>");
}

fn sort_symbol_refs(symbols: &mut Vec<&SymbolNode>) {
    symbols.sort_by(|a, b| {
        a.ordinal
            .cmp(&b.ordinal)
            .then_with(|| a.kind.cmp(&b.kind))
            .then_with(|| a.name.cmp(&b.name))
            .then_with(|| a.id.cmp(&b.id))
    });
}

fn render_symbol_detail(snapshot: &ExplorerSnapshot, selected: Option<&SymbolNode>) -> String {
    let Some(symbol) = selected else {
        return "<div class=\"empty\"><h2>No symbol selected</h2></div>".to_string();
    };
    let path = symbol_path(&snapshot.materialized.graph, symbol)
        .into_iter()
        .map(|part| escape_html(&part))
        .collect::<Vec<_>>()
        .join(" / ");
    let body = symbol
        .body
        .as_deref()
        .map(|body| render_code_block(body, "code", MAX_BODY_BYTES))
        .unwrap_or_else(|| {
            "<pre class=\"code\"><code>No body for this symbol.</code></pre>".to_string()
        });
    let metadata = serde_json::to_string_pretty(&symbol.metadata)
        .map(|text| escape_html(&text))
        .unwrap_or_else(|_| "{}".to_string());

    format!(
        r#"<div class="section-title">Symbol</div>
<h2>{name}</h2>
<div class="chips">
  <span>{kind}</span>
  <span>{id}</span>
  <span>{path}</span>
</div>
<h3>Body</h3>
{body}
<h3>Metadata</h3>
<pre class="code compact"><code>{metadata}</code></pre>"#,
        name = escape_html(&symbol.name),
        kind = escape_html(&symbol.kind),
        id = symbol.id,
        path = path,
        body = body,
        metadata = metadata
    )
}

fn render_inspector(snapshot: &ExplorerSnapshot, selected: Option<&SymbolNode>) -> String {
    let Some(symbol) = selected else {
        return "<div class=\"section-title\">Inspector</div><p class=\"muted\">No current selection.</p>".to_string();
    };

    format!(
        r#"<div class="section-title">Inspector</div>
<h3>References</h3>
{references}
<h3>Symbol History</h3>
{history}"#,
        references = render_references(snapshot, symbol),
        history = render_symbol_history(snapshot, symbol.id)
    )
}

fn render_references(snapshot: &ExplorerSnapshot, symbol: &SymbolNode) -> String {
    let references = snapshot.materialized.graph.find_references(symbol.id);
    if references.is_empty() {
        return "<p class=\"muted\">No references for this symbol.</p>".to_string();
    }
    let mut out = String::from("<ul class=\"list\">");
    for reference in references {
        out.push_str(&render_reference(snapshot, symbol.id, reference));
    }
    out.push_str("</ul>");
    out
}

fn render_reference(
    snapshot: &ExplorerSnapshot,
    selected_id: Uuid,
    reference: &ReferenceNode,
) -> String {
    let (direction, other_id) = if reference.from_symbol_id == selected_id {
        ("out", reference.to_symbol_id)
    } else {
        ("in", reference.from_symbol_id)
    };
    let other = snapshot.materialized.graph.symbols.get(&other_id);
    let other_label = other
        .map(|symbol| format!("{} {}", symbol.kind, symbol.name))
        .unwrap_or_else(|| other_id.to_string());
    let link_attrs = symbol_link_attrs(snapshot, other_id);
    format!(
        r#"<li><span class="pill">{direction}</span> <span>{kind}</span> <a {link_attrs}>{other}</a></li>"#,
        direction = direction,
        kind = escape_html(&reference.kind),
        link_attrs = link_attrs,
        other = escape_html(&other_label)
    )
}

fn render_symbol_history(snapshot: &ExplorerSnapshot, symbol_id: Uuid) -> String {
    let mut matching = snapshot
        .operation_records
        .iter()
        .filter(|record| operation_mentions_symbol(&record.operation, symbol_id))
        .collect::<Vec<_>>();
    matching.reverse();
    if matching.is_empty() {
        return "<p class=\"muted\">No operation history for this symbol.</p>".to_string();
    }
    let mut out = String::from("<ol class=\"timeline\">");
    for record in matching.into_iter().take(16) {
        out.push_str(&render_operation_row(snapshot, record));
    }
    out.push_str("</ol>");
    out
}

fn render_files(
    context: &ExplorerContext,
    snapshot: &ExplorerSnapshot,
    selected: Option<&SymbolNode>,
) -> String {
    let selected_root =
        selected.and_then(|symbol| root_symbol(&snapshot.materialized.graph, symbol));
    let Some(selected_root) = selected_root else {
        return "<p class=\"muted\">No rendered source for this selection.</p>".to_string();
    };
    let selected_path = file_symbol_path(selected_root);
    let slice = context.storage.plugin().render_slice(
        &snapshot.materialized.graph,
        snapshot.materialized.operations.len().to_string(),
        vec![selected_root.id],
    );
    let selected_file = slice
        .files
        .into_iter()
        .find(|file| file.path == selected_path);

    let mut out = String::new();
    if let Some(file) = selected_file {
        out.push_str(&format!(
            r#"<h3>{}</h3>{}"#,
            escape_html(&file.path),
            render_code_block(&file.content, "code file", MAX_SOURCE_BYTES)
        ));
    } else {
        out.push_str("<p class=\"muted\">No rendered source for this selection.</p>");
    }
    out
}

fn render_operations(snapshot: &ExplorerSnapshot) -> String {
    if snapshot.materialized.operations.is_empty() {
        return "<p class=\"muted\">No visible operations for this snapshot.</p>".to_string();
    }
    let mut out = String::from("<ol class=\"timeline\">");
    for record in snapshot.materialized.operations.iter().rev().take(32) {
        out.push_str(&render_operation_row(snapshot, record));
    }
    out.push_str("</ol>");
    out
}

fn render_operation_row(snapshot: &ExplorerSnapshot, record: &OperationRecord) -> String {
    let branch = snapshot
        .branch_names
        .get(&record.branch_id)
        .cloned()
        .unwrap_or_else(|| "unknown".to_string());
    format!(
        r#"<li><span class="op-type">{op_type}</span> <span>{summary}</span><small>{branch} · #{position}</small></li>"#,
        op_type = record.operation.op_type(),
        summary = escape_html(&operation_summary(&record.operation)),
        branch = escape_html(&branch),
        position = record.position
    )
}

fn operation_summary(operation: &Operation) -> String {
    match operation {
        Operation::CreateSymbol {
            kind,
            name,
            symbol_id,
            ..
        } => format!("create {kind} {name} ({symbol_id})"),
        Operation::DeleteSymbol { symbol_id } => format!("delete symbol {symbol_id}"),
        Operation::MoveSymbol {
            symbol_id,
            new_parent_id,
        } => match new_parent_id {
            Some(parent) => format!("move symbol {symbol_id} to {parent}"),
            None => format!("move symbol {symbol_id} to top level"),
        },
        Operation::UpdateSymbol {
            symbol_id,
            name,
            body,
            metadata,
        } => {
            let mut fields = Vec::new();
            if name.is_some() {
                fields.push("name");
            }
            if body.is_some() {
                fields.push("body");
            }
            if metadata.is_some() {
                fields.push("metadata");
            }
            format!("update symbol {symbol_id} ({})", fields.join(", "))
        }
        Operation::CreateReference {
            from_symbol_id,
            to_symbol_id,
            kind,
            ..
        } => format!("create {kind} reference {from_symbol_id} -> {to_symbol_id}"),
        Operation::DeleteReference { reference_id } => format!("delete reference {reference_id}"),
    }
}

fn operation_mentions_symbol(operation: &Operation, symbol_id: Uuid) -> bool {
    operation.write_symbols().contains(&symbol_id)
        || operation
            .reference_endpoints()
            .is_some_and(|(from, to)| from == symbol_id || to == symbol_id)
}

fn symbol_path(graph: &SemanticGraph, symbol: &SymbolNode) -> Vec<String> {
    let mut parts = vec![symbol.name.clone()];
    let mut parent_id = symbol.parent_id;
    while let Some(id) = parent_id {
        let Some(parent) = graph.symbols.get(&id) else {
            break;
        };
        parts.push(parent.name.clone());
        parent_id = parent.parent_id;
    }
    parts.reverse();
    parts
}

fn root_symbol<'a>(graph: &'a SemanticGraph, symbol: &'a SymbolNode) -> Option<&'a SymbolNode> {
    let mut current = symbol;
    while let Some(parent_id) = current.parent_id {
        current = graph.symbols.get(&parent_id)?;
    }
    Some(current)
}

fn file_symbol_path(symbol: &SymbolNode) -> String {
    metadata_string(&symbol.metadata, "path").unwrap_or_else(|| symbol.name.clone())
}

fn symbol_link_attrs(snapshot: &ExplorerSnapshot, symbol_id: Uuid) -> String {
    let href = link_to(&snapshot.branch_name, snapshot.as_of, Some(symbol_id));
    let fragment_href = fragment_link_to(&snapshot.branch_name, snapshot.as_of, Some(symbol_id));
    format!(
        r#"href="{href}" data-symbol-id="{symbol_id}" hx-get="{fragment_href}" hx-push-url="{href}""#,
        href = escape_attr(&href),
        fragment_href = escape_attr(&fragment_href),
    )
}

fn link_to(branch: &str, as_of: Option<i64>, symbol: Option<Uuid>) -> String {
    link_with_path("/", branch, as_of, symbol)
}

fn fragment_link_to(branch: &str, as_of: Option<i64>, symbol: Option<Uuid>) -> String {
    link_with_path("/symbol", branch, as_of, symbol)
}

fn link_with_path(path: &str, branch: &str, as_of: Option<i64>, symbol: Option<Uuid>) -> String {
    let mut params = vec![format!("branch={}", percent_encode(branch))];
    if let Some(as_of) = as_of {
        params.push(format!("as_of={as_of}"));
    }
    if let Some(symbol) = symbol {
        params.push(format!("symbol={symbol}"));
    }
    format!("{path}?{}", params.join("&"))
}

fn render_code_block(value: &str, class_name: &str, max_bytes: usize) -> String {
    let (value, omitted) = display_excerpt(value, max_bytes);
    let note = omitted
        .map(|bytes| {
            format!(
                r#"<p class="muted truncated-note">Showing the first {} bytes; {} bytes omitted.</p>"#,
                value.len(),
                bytes
            )
        })
        .unwrap_or_default();
    format!(
        r#"<pre class="{class_name}"><code>{code}</code></pre>{note}"#,
        class_name = class_name,
        code = escape_html(value),
        note = note,
    )
}

fn display_excerpt(value: &str, max_bytes: usize) -> (&str, Option<usize>) {
    if value.len() <= max_bytes {
        return (value, None);
    }
    let mut end = max_bytes;
    while end > 0 && !value.is_char_boundary(end) {
        end -= 1;
    }
    (&value[..end], Some(value.len() - end))
}

fn explorer_url(addr: SocketAddr) -> String {
    let host = match addr.ip() {
        IpAddr::V4(ip) if ip.is_unspecified() => IpAddr::V4(Ipv4Addr::LOCALHOST).to_string(),
        IpAddr::V6(ip) if ip.is_unspecified() => Ipv4Addr::LOCALHOST.to_string(),
        IpAddr::V6(ip) => format!("[{ip}]"),
        ip => ip.to_string(),
    };
    format!("http://{host}:{}", addr.port())
}

async fn write_status_file(context: &ExplorerContext, url: &str) -> Result<()> {
    let dir = context.root.join(".bonhomme");
    fs::create_dir_all(&dir).await?;
    let path = dir.join("explorer.json");
    let payload = json!({
        "pid": std::process::id(),
        "url": url,
        "repository": context.repository_name,
        "defaultBranch": context.default_branch,
        "root": context.root,
        "config": context.config_label,
        "storage": context.database_label,
    });
    fs::write(path, serde_json::to_string_pretty(&payload)?).await?;
    Ok(())
}

fn open_browser(url: &str) {
    let result = if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else if cfg!(target_os = "windows") {
        Command::new("cmd").args(["/C", "start", url]).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };

    if let Err(error) = result {
        eprintln!("could not open browser: {error}");
    }
}

pub fn config_label(root: &Path) -> String {
    let path = root.join("bonhomme.toml");
    if path.is_file() {
        path.display().to_string()
    } else {
        format!("defaults from {}", root.display())
    }
}

pub fn database_label(database_url: &str) -> String {
    if database_url.starts_with("postgres://") || database_url.starts_with("postgresql://") {
        "postgres".to_string()
    } else if database_url == ":memory:" {
        "in-memory Turso".to_string()
    } else if database_url.starts_with("turso:")
        || database_url.starts_with("sqlite:")
        || database_url.starts_with("file:")
    {
        "embedded Turso".to_string()
    } else {
        "custom".to_string()
    }
}

fn percent_encode(value: &str) -> String {
    let mut out = String::new();
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

fn escape_html(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn escape_attr(value: &str) -> String {
    escape_html(value)
}

const SCRIPTS: &str = r#"<script defer src="https://unpkg.com/htmx.org@2.0.4/dist/htmx.min.js"></script>
<script defer>
document.addEventListener("DOMContentLoaded", () => {
  document.body.addEventListener("htmx:beforeRequest", (event) => {
    const source = event.detail.elt;
    const symbolId = source && source.getAttribute("data-symbol-id");
    if (!symbolId) return;

    document.querySelectorAll(".tree-link.selected").forEach((link) => {
      link.classList.remove("selected");
      link.removeAttribute("aria-current");
    });

    const treeLink = document.querySelector(`.tree-link[data-symbol-id="${symbolId}"]`);
    if (treeLink) {
      treeLink.classList.add("selected");
      treeLink.setAttribute("aria-current", "true");
      treeLink.scrollIntoView({ block: "nearest" });
    }

    const symbolInput = document.querySelector('input[name="symbol"]');
    if (symbolInput) symbolInput.value = symbolId;
  });
});
</script>"#;

const STYLES: &str = r#"<style>
:root {
  color-scheme: light dark;
  --bg: #f6f8fa;
  --panel: #ffffff;
  --border: #d0d7de;
  --text: #1f2328;
  --muted: #59636e;
  --accent: #0969da;
  --accent-soft: #ddf4ff;
  --accent-strong: #0550ae;
  --code: #ffffff;
  --code-border: #d8dee4;
  --control-bg: #ffffff;
  --button-text: #ffffff;
  --op-type: #8250df;
}
@media (prefers-color-scheme: dark) {
  :root {
    --bg: #0d1117;
    --panel: #161b22;
    --border: #30363d;
    --text: #e6edf3;
    --muted: #8b949e;
    --accent: #58a6ff;
    --accent-soft: #1f6feb33;
    --accent-strong: #79c0ff;
    --code: #111820;
    --code-border: #3d444d;
    --control-bg: #0d1117;
    --button-text: #0d1117;
    --op-type: #d2a8ff;
  }
}
* { box-sizing: border-box; }
html {
  height: 100%;
}
body {
  margin: 0;
  min-height: 100%;
  height: 100dvh;
  display: grid;
  grid-template-rows: auto minmax(0, 1fr);
  overflow: hidden;
  background: var(--bg);
  color: var(--text);
  font: 14px/1.45 -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}
a { color: var(--accent); text-decoration: none; }
a:hover { text-decoration: underline; }
.topbar {
  display: grid;
  grid-template-columns: minmax(180px, max-content) minmax(0, 1fr);
  align-items: center;
  gap: 24px;
  min-height: 82px;
  padding: 12px 22px;
  border-bottom: 1px solid var(--border);
  background: var(--panel);
}
.title-block {
  min-width: 0;
  display: grid;
  gap: 1px;
}
.eyebrow {
  color: var(--muted);
  font-size: 11px;
  font-weight: 700;
  letter-spacing: .02em;
  text-transform: uppercase;
}
h1, h2, h3 { margin: 0; }
h1 { font-size: 24px; line-height: 1.08; }
h2 { font-size: 20px; margin-bottom: 10px; }
h3 { font-size: 13px; margin: 18px 0 8px; color: var(--muted); text-transform: uppercase; }
.topbar-tools {
  min-width: 0;
  display: flex;
  flex-wrap: wrap;
  justify-content: flex-end;
  align-items: center;
  gap: 10px 18px;
}
.meta, .chips {
  display: flex;
  flex-wrap: wrap;
  gap: 6px;
  align-items: center;
}
.meta span, .chips span, .pill {
  border: 1px solid var(--border);
  background: var(--bg);
  border-radius: 999px;
  padding: 2px 8px;
  color: var(--muted);
  white-space: nowrap;
}
.meta span {
  background: transparent;
  font-size: 13px;
  line-height: 1.45;
}
.top-controls {
  display: flex;
  flex-wrap: wrap;
  align-items: center;
  justify-content: flex-end;
  gap: 6px;
}
.field {
  display: block;
}
.visually-hidden {
  position: absolute;
  width: 1px;
  height: 1px;
  padding: 0;
  margin: -1px;
  overflow: hidden;
  clip: rect(0, 0, 0, 0);
  white-space: nowrap;
  border: 0;
}
select, input, button, .button {
  min-height: 34px;
  border: 1px solid var(--border);
  border-radius: 6px;
  background: var(--control-bg);
  color: var(--text);
  padding: 5px 10px;
  font: inherit;
}
select[name="branch"] {
  min-width: 96px;
}
input[name="as_of"] {
  width: 96px;
}
button, .button {
  cursor: pointer;
  background: var(--accent);
  color: var(--button-text);
  border-color: var(--accent);
  font-weight: 600;
}
.button.secondary {
  background: var(--control-bg);
  color: var(--accent);
}
.explorer-layout {
  display: grid;
  grid-template-columns: minmax(220px, 280px) minmax(0, 980px);
  gap: 32px;
  align-items: stretch;
  min-height: 0;
  overflow: hidden;
  padding: 24px 24px 0;
}
.symbol-rail {
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
  padding-right: 24px;
  padding-bottom: 40px;
  border-right: 1px solid var(--border);
}
.reading-column {
  min-width: 0;
  min-height: 0;
  overflow-y: auto;
  overscroll-behavior: contain;
  scrollbar-gutter: stable;
  display: grid;
  gap: 28px;
  padding-right: 8px;
  padding-bottom: 40px;
}
.selection-panel {
  min-width: 0;
  display: grid;
  gap: 28px;
}
.selection-panel.htmx-request {
  opacity: 0.62;
}
.content-section {
  min-width: 0;
  padding-top: 18px;
  border-top: 1px solid var(--border);
}
.identity-section {
  padding-top: 0;
  border-top: 0;
}
.content-grid {
  display: grid;
  grid-template-columns: minmax(0, 1fr) minmax(0, 1fr);
  gap: 32px;
  align-items: start;
  padding-top: 18px;
  border-top: 1px solid var(--border);
}
.content-grid .content-section {
  padding-top: 0;
  border-top: 0;
}
.section-title {
  color: var(--muted);
  font-size: 12px;
  font-weight: 700;
  letter-spacing: 0;
  text-transform: uppercase;
  margin-bottom: 10px;
}
.symbol-tree, .symbol-tree ul, .list, .timeline {
  list-style: none;
  margin: 0;
  padding: 0;
}
.symbol-tree ul { margin-left: 12px; }
.tree-link {
  display: grid;
  grid-template-columns: auto minmax(0, 1fr);
  gap: 7px;
  align-items: center;
  min-height: 26px;
  padding: 3px 6px;
  border-radius: 6px;
  color: var(--text);
}
.tree-link span:last-child {
  min-width: 0;
  overflow-wrap: anywhere;
}
.tree-link.selected { background: var(--accent-soft); color: var(--accent-strong); font-weight: 700; }
.tree-link.htmx-request { opacity: 0.65; }
.kind {
  color: var(--muted);
  font-size: 11px;
  font-weight: 700;
  text-transform: uppercase;
}
.code {
  margin: 0;
  padding: 12px;
  border: 1px solid var(--code-border);
  border-radius: 6px;
  background: var(--code);
  box-shadow: inset 0 1px 0 color-mix(in srgb, var(--panel) 65%, transparent);
  font: 12px/1.5 ui-monospace, SFMono-Regular, Menlo, Consolas, monospace;
  white-space: pre-wrap;
  overflow-wrap: anywhere;
}
.list li, .timeline li {
  display: grid;
  gap: 3px;
  padding: 8px 0;
  border-top: 1px solid var(--border);
}
.list li:first-child, .timeline li:first-child { border-top: 0; }
.timeline small { color: var(--muted); }
.op-type {
  color: var(--op-type);
  font-weight: 700;
}
.muted, .empty p { color: var(--muted); }
.truncated-note {
  margin: 6px 0 0;
}
details { margin-top: 12px; }
summary { cursor: pointer; color: var(--accent); font-weight: 600; }
.environment {
  padding-top: 18px;
  border-top: 1px solid var(--border);
  color: var(--muted);
}
.environment dl {
  display: grid;
  gap: 6px;
  margin: 10px 0 0;
}
.environment dl div {
  display: grid;
  grid-template-columns: 72px minmax(0, 1fr);
  gap: 10px;
}
.environment dt {
  font-weight: 700;
  text-transform: uppercase;
  font-size: 11px;
}
.environment dd {
  margin: 0;
  overflow-wrap: anywhere;
}
.error-page {
  max-width: 760px;
  margin: 60px auto;
  padding: 24px;
  border: 1px solid var(--border);
  border-radius: 8px;
  background: var(--panel);
}
@media (max-width: 1100px) {
  body {
    min-height: 100dvh;
    height: auto;
    display: block;
    overflow: auto;
  }
  .explorer-layout, .content-grid { grid-template-columns: 1fr; }
  .explorer-layout {
    overflow: visible;
    padding-bottom: 40px;
  }
  .topbar {
    grid-template-columns: 1fr;
    align-items: start;
  }
  .topbar-tools, .top-controls { justify-content: flex-start; }
  .symbol-rail {
    overflow: visible;
    padding-right: 0;
    padding-bottom: 18px;
    border-right: 0;
    border-bottom: 1px solid var(--border);
  }
  .reading-column {
    overflow: visible;
    padding-right: 0;
    padding-bottom: 0;
  }
}
</style>"#;
