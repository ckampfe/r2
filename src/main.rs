#![forbid(unsafe_code)]
// TODO
// - [ ] add a feed
// - [ ] nav on desktop (drawer?)
// - [ ] nav on mobile (???)
// - [ ] refresh individual feed
// - [ ] refresh all feeds
// - [ ] figure out why <hr> won't show up at bottom of entry
// - [x] on feed_show: show read entries
// - [x] on feed_show: show unread entries
// - [x] on feed_show: show all entries
// - [ ] on feed_show: navigate back to index
// - [ ] on entry_show: navigate back to feed
// - [ ] on entry_show: navigate to other entry in feed
// - [ ] on entry_show: the pub_date should look better
// - [x] on entry_show: mark read
// - [x] on entry_show: mark unread
// - [ ] code to pick a default database location

use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use clap::Parser;
use maud::{PreEscaped, html};
use serde::Deserialize;
use sqlx::Connection;
use sqlx::Executor;
use sqlx::Sqlite;
use sqlx::prelude::FromRow;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::instrument;

macro_rules! layout {
    ($content:expr) => {
        maud::html! {
            (maud::DOCTYPE)
            head {
                meta charset="UTF-8";
                meta name="viewport" content="width=device-width, initial-scale=1.0";
                title {
                    "russweb"
                }
                script
                    src="https://cdn.jsdelivr.net/npm/htmx.org@2.0.6/dist/htmx.min.js"
                    integrity="sha384-Akqfrbj/HpNVo8k11SXBb6TlBWmXXlYQrCSqEWmyKJe+hDm3Z/B2WVG4smwBkRVm"
                    crossorigin="anonymous" {}
                link href="/output.css" rel="stylesheet";
            }
            body {
                div class="grid container mx-auto px-4" {
                    ($content)
                }
            }
        }
    };
}

#[instrument(skip(state))]
async fn home(State(state): State<Arc<Mutex<AppState>>>) -> Result<impl IntoResponse, AppError> {
    #[derive(FromRow)]
    struct Feed {
        id: i64,
        title: String,
        unread_entries: i64,
        read_entries: i64,
    }

    let state = state.lock().await;

    let mut conn = state.pool.acquire().await?;

    let feeds: Vec<Feed> = sqlx::query_as(
        "
    select
        feeds.id,
        feeds.title,
        sum(case when entries.read_at then 0 else 1 end) as unread_entries,
        sum(case when entries.read_at then 1 else 0 end) as read_entries
    from feeds
    inner join entries
        on entries.feed_id = feeds.id
    group by feeds.id
    order by feeds.title asc
    ",
    )
    .fetch_all(&mut *conn)
    .await?;

    //   <div class="drawer lg:drawer-open">
    //      <input id="my-drawer-2" type="checkbox" class="drawer-toggle" />
    //   <div class="drawer-content flex flex-col items-center justify-center">
    //     <!-- Page content here -->
    //     <label for="my-drawer-2" class="btn btn-primary drawer-button lg:hidden">
    //       Open drawer
    //     </label>
    //   </div>
    //   <div class="drawer-side">
    //     <label for="my-drawer-2" aria-label="close sidebar" class="drawer-overlay"></label>
    //     <ul class="menu bg-base-200 text-base-content min-h-full w-80 p-4">
    //       <!-- Sidebar content here -->
    //       <li><a>Sidebar Item 1</a></li>
    //       <li><a>Sidebar Item 2</a></li>
    //     </ul>
    //   </div>
    // </div>

    Ok(layout! {
        html! {
            div class="p-4" {
                table class="table" {
                    thead {
                        tr {
                            th { "Feed" }
                            th { "Most recent entry" }
                            th { "Last refreshed" }
                            th { "Unread entries" }
                            th { "Read entries" }
                        }
                    }
                    tbody {
                        @for feed in feeds {
                            tr {
                                td {
                                    a class="link" href=(format!("/feeds/{}", feed.id)) {
                                        (feed.title)
                                    }
                                }
                                td { "123" }
                                td { "1213" }
                                td { (feed.unread_entries) }
                                td { (feed.read_entries) }
                            }
                        }
                    }
                }
            }
        }
    })
}

#[derive(Deserialize, Debug)]
struct FeedShowParams {
    entries_visibility: Option<EntriesVisibility>,
}

#[derive(Clone, Copy, Deserialize, Debug, Default, PartialEq)]
enum EntriesVisibility {
    #[default]
    #[serde(rename = "unread")]
    Unread,
    #[serde(rename = "read")]
    Read,
    #[serde(rename = "all")]
    All,
}

impl EntriesVisibility {
    fn is_read(&self) -> bool {
        *self == Self::Read
    }

    fn is_all(&self) -> bool {
        *self == Self::All
    }
}

#[instrument(skip(state))]
async fn feed_show(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(feed_id): Path<i64>,
    Query(params): Query<FeedShowParams>,
) -> Result<impl IntoResponse, AppError> {
    // id INTEGER PRIMARY KEY AUTOINCREMENT,
    // feed_id INTEGER,
    // title TEXT,
    // author TEXT,
    // pub_date TIMESTAMP,
    // description TEXT,
    // content TEXT,
    // link TEXT,
    // read_at TIMESTAMP,
    // inserted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
    // updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
    #[derive(FromRow)]
    struct Feed {
        title: String,
    }

    #[derive(FromRow)]
    struct Entry {
        id: i64,
        title: String,
        pub_date: chrono::DateTime<chrono::Utc>,
        // description: String,
        // content: String,
        link: String,
        read_at: Option<chrono::DateTime<chrono::Utc>>,
    }

    let state = state.lock().await;

    let mut conn = state.pool.acquire().await?;

    let feed: Feed = sqlx::query_as(
        "
        select
            title
        from feeds
        where id = ?",
    )
    .bind(feed_id)
    .fetch_one(&mut *conn)
    .await?;

    let mut qb: sqlx::QueryBuilder<Sqlite> = sqlx::QueryBuilder::new(
        "
        select
            id,
            title,
            pub_date,
            link,
            read_at
        from entries
        where feed_id = ",
    );

    qb.push_bind(feed_id);

    if let Some(entries_visibility) = params.entries_visibility {
        match entries_visibility {
            EntriesVisibility::Unread => {
                qb.push(" and read_at is null ");
            }
            EntriesVisibility::Read => {
                qb.push(" and read_at is not null ");
            }
            EntriesVisibility::All => {}
        }
    } else {
        qb.push(" and read_at is null ");
    }

    qb.push(" order by pub_date desc ");

    let entries: Vec<Entry> = qb.build_query_as().fetch_all(&mut *conn).await?;

    // dbg!(query);

    // let entries: Vec<Entry> = sqlx::query_as(
    //     query, //     "
    //           // select
    //           //     id,
    //           //     title,
    //           //     pub_date,
    //           //     link,
    //           //     read_at
    //           // from entries
    //           // where feed_id = ?
    //           // order by pub_date desc
    //           // ",
    // )
    // .bind(feed_id)
    // .fetch_all(&mut *conn)
    // .await?;

    Ok(layout! {
        html! {
            div class="p-2" {
                h1 {
                    (feed.title)
                }
                a class="link p-2" href=(format!("/feeds/{feed_id}/refresh")) {
                    "Refresh feed"
                }
                {
                    @match params.entries_visibility {
                        Some(v) => {
                            @match v {
                                EntriesVisibility::Unread => {
                                    a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=read")) {
                                        "View read entries"
                                    }
                                    a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=all")) {
                                        "View all entries"
                                    }
                                },
                                EntriesVisibility::Read => {
                                    a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=unread")) {
                                        "View unread entries"
                                    }
                                    a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=all")) {
                                        "View all entries"
                                    }
                                },
                                EntriesVisibility::All => {
                                    a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=unread")) {
                                        "View unread entries"
                                    }
                                    a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=read")) {
                                        "View read entries"
                                    }
                                },
                            }
                        },
                        None => {
                            a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=read")) {
                                "View read entries"
                            }
                            a class="link p-2" href=(format!("/feeds/{feed_id}?entries_visibility=all")) {
                                "View all entries"
                            }
                        },
                    }
                }
                table class="table" {
                    thead {
                        tr {
                            th { "Title" }
                            th { "Publication date" }
                            @if params.entries_visibility.map(|v| v.is_read() || v.is_all()).unwrap_or(false) {
                                th { "Read at" }
                            }
                            th { "" }
                        }
                    }
                    tbody {
                        @for entry in entries {
                            tr {
                                td {
                                    a class="link" href=(format!("/entries/{}", entry.id)) {
                                        (entry.title)
                                    }
                                }
                                td { (entry.pub_date) }
                                @if params.entries_visibility.map(|v| v.is_read() || v.is_all()).unwrap_or(false) {
                                    td { (entry.read_at.map(|dt| dt.to_string()).unwrap_or_else(String::new)) }
                                }
                                td {
                                    a
                                        class="link"
                                        href=(entry.link)
                                        target="_blank"
                                    {
                                        "View original"
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    })
}

#[instrument(skip(state))]
async fn entry_show(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(entry_id): Path<i64>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(FromRow)]
    struct Entry {
        feed_id: i64,
        title: String,
        author: String,
        description: String,
        content: String,
        pub_date: String,
        link: String,
        read_at: Option<String>,
    }

    let state = state.lock().await;

    let mut conn = state.pool.acquire().await?;

    let entry: Entry = sqlx::query_as(
        "
        select
            feed_id,
            title,
            author,
            description,
            content,
            pub_date,
            link,
            read_at
        from entries
        where id = ?
        ",
    )
    .bind(entry_id)
    .fetch_one(&mut *conn)
    .await?;

    let content = if entry.content.len() >= entry.description.len() {
        entry.content
    } else {
        entry.description
    };

    let cleaned = ammonia::clean(&content);

    Ok(layout! {
        html! {
            div class="grid grid-cols-1 justify-items-center p-4" {
                div class="p-4" {
                    article class="prose" {
                        h2 {
                            (entry.title)
                        }
                        h5 {
                            (entry.pub_date)
                        }
                        (PreEscaped(cleaned))
                    }
                }

                div class="divider" {}

                div {
                    a class="link p-2" href=(format!("/feeds/{}", entry.feed_id)) {
                        "Back"
                    }
                    @if entry.read_at.is_none() {
                        a
                            class="link p-2"
                            hx-put=(format!("/entries/{}?action=toggle_read_unread", entry_id))
                            hx-swap="innerHTML"
                        {
                            "Mark read"
                        }
                    } @else {
                        a
                            class="link p-2"
                            hx-put=(format!("/entries/{}?action=toggle_read_unread", entry_id))
                            hx-swap="innerHTML"
                        {
                            "Mark unread"
                        }
                    }
                    a
                        class="link p-2"
                        href=(entry.link)
                        target="_blank"
                    {
                        "View original"
                    }
                }
            }
        }
    })
}

// 1. refresh entry
// 2. mark read
// 3. mark unread
#[derive(Deserialize, Debug)]
struct EntryUpdateParams {
    action: EntryUpdateAction,
}

#[derive(Deserialize, Debug)]
enum EntryUpdateAction {
    #[serde(rename = "refresh")]
    Refresh,
    #[serde(rename = "toggle_read_unread")]
    ToggleReadUnread,
}

#[instrument(skip(state))]
async fn entry_update(
    State(state): State<Arc<Mutex<AppState>>>,
    Path(entry_id): Path<i64>,
    Query(params): Query<EntryUpdateParams>,
) -> Result<impl IntoResponse, AppError> {
    match params.action {
        EntryUpdateAction::Refresh => {
            let state = state.lock().await;

            let mut conn = state.pool.acquire().await?;

            let (read_at,): (String,) = sqlx::query_as(
                "
                update entries
                set read_at = ?1
                where entry_id = ?2
                returning read_at
                ",
            )
            .bind(chrono::Utc::now())
            .bind(entry_id)
            .fetch_one(&mut *conn)
            .await?;

            dbg!(read_at);

            Ok(html! {
                "ok"
            })
        }
        EntryUpdateAction::ToggleReadUnread => {
            let state = state.lock().await;

            let mut conn = state.pool.acquire().await?;

            let mut tx = conn.begin_with("BEGIN IMMEDIATE").await?;

            let (read_at,): (Option<String>,) = sqlx::query_as(
                "
            select
                read_at
            from entries
            where id = ?
            ",
            )
            .bind(entry_id)
            .fetch_one(&mut *tx)
            .await?;

            let out = if read_at.is_some() {
                sqlx::query(
                    "
                update entries
                set read_at = null
                where id = ?
                ",
                )
                .bind(entry_id)
                .execute(&mut *tx)
                .await?;

                html! {
                    "Mark read"
                }
            } else {
                sqlx::query(
                    "
                update entries
                set read_at = ?
                where id = ?
                ",
                )
                .bind(chrono::Utc::now())
                .bind(entry_id)
                .execute(&mut *tx)
                .await?;

                html! {
                    "Mark unread"
                }
            };

            tx.commit().await?;

            Ok(out)
        }
    }
}

#[instrument(skip(conn))]
async fn initialize_db(conn: &mut sqlx::SqliteConnection) -> anyhow::Result<()> {
    // in_transaction(conn, |tx| {
    let mut tx = conn.begin().await?;

    // let schema_version: u64 = tx.pragma_query_value(None, "user_version", |row| row.get(0))?;
    let (schema_version,): (u64,) = sqlx::query_as("PRAGMA user_version")
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or((0,));

    if schema_version == 0 {
        // tx.pragma_update(None, "user_version", 1)?;
        tx.execute("PRAGMA user_version=1").await?;

        sqlx::query(
            "CREATE TABLE IF NOT EXISTS feeds (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        title TEXT,
        feed_link TEXT,
        link TEXT,
        feed_kind TEXT,
        refreshed_at TIMESTAMP,
        inserted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
        updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        )
        .execute(&mut *tx)
        .await?;

        // tx.execute(
        sqlx::query(
            "CREATE TABLE IF NOT EXISTS entries (
        id INTEGER PRIMARY KEY AUTOINCREMENT,
        feed_id INTEGER,
        title TEXT,
        author TEXT,
        pub_date TIMESTAMP,
        description TEXT,
        content TEXT,
        link TEXT,
        read_at TIMESTAMP,
        inserted_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP,
        updated_at TIMESTAMP DEFAULT CURRENT_TIMESTAMP
        )",
        )
        .execute(&mut *tx)
        .await?;

        // tx.execute(
        sqlx::query(
            "CREATE INDEX IF NOT EXISTS entries_feed_id_and_pub_date_and_inserted_at_index
        ON entries (feed_id, pub_date, inserted_at)",
        )
        .execute(&mut *tx)
        .await?;
    }

    if schema_version <= 1 {
        // tx.pragma_update(None, "user_version", 2)?;
        tx.execute("PRAGMA user_version=2").await?;

        sqlx::query("ALTER TABLE feeds ADD COLUMN latest_etag TEXT")
            .execute(&mut *tx)
            .await?;
    }

    if schema_version <= 2 {
        // tx.pragma_update(None, "user_version", 3)?;
        tx.execute("PRAGMA user_version=3").await?;

        sqlx::query("CREATE UNIQUE INDEX IF NOT EXISTS feeds_feed_link ON feeds (feed_link)")
            .execute(&mut *tx)
            .await?;
    }

    tx.commit().await?;

    Ok(())
}

struct AppError(anyhow::Error);

// Tell axum how to convert `AppError` into a response.
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            format!("Something went wrong: {}", self.0),
        )
            .into_response()
    }
}

impl<E> From<E> for AppError
where
    E: Into<anyhow::Error>,
{
    fn from(err: E) -> Self {
        Self(err.into())
    }
}

#[derive(Debug)]
struct AppState {
    pool: sqlx::Pool<Sqlite>,
    http_client: reqwest::Client,
}

#[derive(Debug, Parser)]
struct Config {
    #[arg(long, env, default_value = "feeds.db")]
    database: String,
    #[arg(long, env, default_value = "3000")]
    port: u16,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .init();

    let config = Config::parse();

    let opts =
        sqlx::sqlite::SqliteConnectOptions::from_str(&format!("sqlite://{}", config.database))?
            .busy_timeout(std::time::Duration::from_secs(5))
            .journal_mode(sqlx::sqlite::SqliteJournalMode::Wal)
            .create_if_missing(true)
            .foreign_keys(true);

    let pool = sqlx::SqlitePool::connect_with(opts).await?;

    let mut conn = pool.acquire().await?;

    initialize_db(&mut conn).await?;

    let http_client = reqwest::Client::new();

    let state = Arc::new(Mutex::new(AppState { pool, http_client }));

    let asset_service = tower_http::services::ServeDir::new("assets");

    let router = Router::new()
        .route("/", get(home))
        .route("/feeds/{feed_id}", get(feed_show))
        .route("/entries/{entry_id}", get(entry_show).put(entry_update))
        .with_state(state)
        .fallback_service(asset_service);

    let listener = tokio::net::TcpListener::bind("localhost:4000").await?;

    axum::serve(listener, router).await?;

    Ok(())
}
