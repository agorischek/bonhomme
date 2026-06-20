//! Probe what the experimental `turso` crate (the Rust SQLite rewrite) actually supports,
//! so the real TursoBackend knows whether it can use RETURNING / ON CONFLICT / transactions
//! or must work around them. Run: `cargo run -p bonhomme-engine --example turso_spike`.
use turso::{Builder, Connection, params};

async fn probe(name: &str, result: anyhow::Result<String>) {
    match result {
        Ok(detail) => println!("  [ ok ] {name:<24} {detail}"),
        Err(error) => println!("  [FAIL] {name:<24} {error}"),
    }
}

async fn returning(conn: &Connection) -> anyhow::Result<String> {
    let mut rows = conn
        .query(
            "INSERT INTO t (id, name, payload) VALUES (?1, ?2, ?3) RETURNING name, created_at",
            params!["r1", "alpha", "{\"k\":1}"],
        )
        .await?;
    match rows.next().await? {
        Some(row) => Ok(format!("returned name={:?}", row.get_value(0)?)),
        None => Ok("no row returned".into()),
    }
}

async fn on_conflict(conn: &Connection) -> anyhow::Result<String> {
    conn.execute("INSERT INTO t (id, name) VALUES (?1, ?2)", params!["c1", "beta"])
        .await?;
    conn.execute(
        "INSERT INTO t (id, name) VALUES (?1, ?2) ON CONFLICT(name) DO UPDATE SET payload = ?3",
        params!["c2", "beta", "updated"],
    )
    .await?;
    Ok("ON CONFLICT DO UPDATE accepted".into())
}

async fn on_conflict_nothing(conn: &Connection) -> anyhow::Result<String> {
    conn.execute(
        "INSERT INTO t (id, name) VALUES (?1, ?2) ON CONFLICT(name) DO NOTHING",
        params!["c3", "beta"],
    )
    .await?;
    Ok("ON CONFLICT DO NOTHING accepted".into())
}

async fn transaction_raw(conn: &Connection) -> anyhow::Result<String> {
    conn.execute("BEGIN", ()).await?;
    conn.execute("INSERT INTO t (id, name) VALUES (?1, ?2)", params!["t1", "gamma"])
        .await?;
    conn.execute("COMMIT", ()).await?;
    Ok("BEGIN/COMMIT accepted".into())
}

async fn aggregate_and_params(conn: &Connection) -> anyhow::Result<String> {
    let mut rows = conn
        .query(
            "SELECT COALESCE(MAX(rowid), 0) + 1 FROM t WHERE name = ?1",
            params!["alpha"],
        )
        .await?;
    let next = match rows.next().await? {
        Some(row) => format!("{:?}", row.get_value(0)?),
        None => "none".into(),
    };
    Ok(format!("MAX()+1 with ?1 param = {next}"))
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("turso crate feature probe:");
    let db = Builder::new_local(":memory:").build().await?;
    let conn = db.connect()?;
    conn.execute(
        "CREATE TABLE t (
            id TEXT PRIMARY KEY,
            name TEXT NOT NULL UNIQUE,
            payload TEXT,
            created_at TEXT NOT NULL DEFAULT CURRENT_TIMESTAMP
        )",
        (),
    )
    .await?;
    println!("  [ ok ] CREATE TABLE");

    probe("INSERT … RETURNING", returning(&conn).await).await;
    probe("ON CONFLICT DO UPDATE", on_conflict(&conn).await).await;
    probe("ON CONFLICT DO NOTHING", on_conflict_nothing(&conn).await).await;
    probe("BEGIN/COMMIT", transaction_raw(&conn).await).await;
    probe("MAX()+1 + ?1 param", aggregate_and_params(&conn).await).await;
    Ok(())
}
