#![forbid(unsafe_code)]
// TODO
// - [x] add a feed
// - [x] errors for adding a feed
// - [ ] nav on desktop (drawer?)
// - [ ] nav on mobile (???)
// - [ ] refresh individual feed
// - [ ] refresh all feeds
// - [ ] figure out why <hr> won't show up at bottom of entry
// - [x] figure out a name for this, is russweb good?
// - [ ] search???
// - [ ] on index: sort by arbitrary columns
// - [x] on feed_show: show read entries
// - [x] on feed_show: show unread entries
// - [x] on feed_show: show all entries
// - [x] on feed_show: navigate back to index
// - [ ] on feed_show: sort on arbitrary columns
// - [x] on entry_show: navigate back to feed
// - [x] on entry_show: navigate to other entry in feed
// - [ ] on entry_show: the pub_date should look better
// - [x] on entry_show: mark read
// - [x] on entry_show: mark unread
// - [x] on entry_show: make sure entry text wraps on mobile
// - [ ] pick a default database location
// - [x] rust-embed for css
// - [ ] set up CI

use ammonia::Url;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::{HeaderMap, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use clap::Parser;
use maud::{PreEscaped, html};
use rust_embed::Embed;
use serde::Deserialize;
use sqlx::prelude::FromRow;
use sqlx::{Connection, Executor, Sqlite};
use std::str::FromStr;
use std::sync::Arc;
use thiserror::Error;
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
                    "r2"
                }
                script
                    src="https://cdn.jsdelivr.net/npm/htmx.org@2.0.6/dist/htmx.min.js"
                    integrity="sha384-Akqfrbj/HpNVo8k11SXBb6TlBWmXXlYQrCSqEWmyKJe+hDm3Z/B2WVG4smwBkRVm"
                    crossorigin="anonymous" {}
                link href="/dist/output.css" rel="stylesheet";
            }
            body {
                div class="grid container mx-auto px-4" {
                    ($content)
                }
                script {
                    "document.body.addEventListener('feedCreateError', function(evt){
                        alert(evt.detail.value);
                    })"
                }
            }
        }
    };
}

#[instrument(skip(state))]
async fn feed_index(
    State(state): State<Arc<Mutex<AppState>>>,
) -> Result<impl IntoResponse, AppError> {
    #[derive(FromRow)]
    struct Feed {
        id: i64,
        title: String,
        unread_entries: i64,
        read_entries: i64,
        most_recent_entry: String,
        refreshed_at: String,
    }

    let state = state.lock().await;

    let mut conn = state.pool.acquire().await?;

    let feeds: Vec<Feed> = sqlx::query_as(
        "
    select
        feeds.id,
        feeds.title,
        sum(case when entries.read_at then 0 else 1 end) as unread_entries,
        sum(case when entries.read_at then 1 else 0 end) as read_entries,
        max(coalesce(entries.pub_date, entries.inserted_at)) as most_recent_entry,
        feeds.refreshed_at
    from feeds
    inner join entries
        on entries.feed_id = feeds.id
    group by feeds.id
    order by feeds.title asc
    ",
    )
    .fetch_all(&mut *conn)
    .await?;

    Ok(layout! {
        html! {
            div class="p-4" {
                a
                    class="link"
                    hx-post="/feeds"
                    hx-prompt="Feed URL"
                    hx-swap="none"
                {
                    "Add feed"
                }
                table class="table" {
                    thead {
                        tr {
                            th { "Feed" }
                            th class="hidden sm:table-cell" { "Most recent entry" }
                            th class="hidden sm:table-cell" { "Last refreshed" }
                            th class="hidden sm:table-cell" { "Unread entries" }
                            th class="hidden sm:table-cell" { "Read entries" }
                        }
                    }
                    tbody {
                        @for feed in feeds {
                            tr {
                                td class="text-center sm:text-left" {
                                    a class="link" href=(format!("/feeds/{}", feed.id)) {
                                        (feed.title)
                                    }
                                }
                                td class="hidden sm:table-cell" { (feed.most_recent_entry) }
                                td class="hidden sm:table-cell" { (feed.refreshed_at) }
                                td class="hidden sm:table-cell" { (feed.unread_entries) }
                                td class="hidden sm:table-cell" { (feed.read_entries) }
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
    #[derive(FromRow)]
    struct Feed {
        title: String,
    }

    #[derive(FromRow)]
    struct Entry {
        id: i64,
        title: String,
        pub_date: String,
        link: String,
        read_at: Option<String>,
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

    let entries: Vec<Entry> = {
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

        qb.build_query_as().fetch_all(&mut *conn).await?
    };

    Ok(layout! {
        html! {
            div class="breadcrumbs text-sm" {
                ul {
                    li {
                        a href="/" {
                            "Feeds"
                        }
                    }
                    li {
                        a href=(format!("/feeds/{}", feed_id)) {
                            (feed.title)
                        }
                    }
                }
            }
            div class="p-2" {
                header class="flex flex-wrap justify-start" {
                    h1 {
                        (feed.title)
                    }
                    div {
                        a class="link p-2" href=(format!("/feeds/{feed_id}/refresh")) {
                            "Refresh feed"
                        }
                    }
                    // class=ml-auto here is a hack to get things to go to the right
                    // there is probably a better way to do this,
                    // will probably reevaluate this nav functionality entirely
                    div class="ml-auto" {
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
                }
                main {
                    table class="table" {
                        thead {
                            tr {
                                th { "Title" }
                                th class="hidden sm:table-cell" { "Publication date" }
                                @if params.entries_visibility.map(|v| v.is_read() || v.is_all()).unwrap_or(false) {
                                    th class="hidden sm:table-cell" { "Read at" }
                                }
                                th class="hidden sm:table-cell" { "" }
                            }
                        }
                        tbody {
                            @for entry in entries {
                                tr {
                                    td class="text-center sm:text-left" {
                                        a class="link" href=(format!("/entries/{}", entry.id)) {
                                            (entry.title)
                                        }
                                    }
                                    td class="hidden sm:table-cell" { (entry.pub_date) }
                                    @if params.entries_visibility.map(|v| v.is_read() || v.is_all()).unwrap_or(false) {
                                        td class="hidden sm:table-cell" { (entry.read_at.unwrap_or_else(String::new)) }
                                    }
                                    td class="hidden sm:table-cell" {
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
        // author: String,
        description: String,
        content: String,
        pub_date: String,
        link: String,
        read_at: Option<String>,
    }

    #[derive(FromRow)]
    struct Feed {
        title: String,
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

    let feed: Feed = sqlx::query_as(
        "
        select
            title
        from feeds
        where id = ?
        ",
    )
    .bind(entry.feed_id)
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
            div class="breadcrumbs text-sm" {
                ul {
                    li {
                        a href="/" {
                            "Feeds"
                        }
                    }
                    li {
                        a href=(format!("/feeds/{}", entry.feed_id)) {
                            (feed.title)
                        }
                    }
                    li {
                        a href=(format!("/entries/{}", entry_id)) {
                            (entry.title)
                        }
                    }
                }
            }
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

            let (_read_at,): (String,) = sqlx::query_as(
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

#[derive(Debug, Error)]
enum FeedCreateError {
    #[error("bad input")]
    BadInput(&'static str),
    #[error("network error")]
    NetworkError(#[from] reqwest::Error),
    #[error("feed parse error")]
    FeedParseError(#[from] feed_rs::parser::ParseFeedError),
    #[error("database error")]
    DatabaseError(#[from] sqlx::Error),
}

async fn do_feed_create(
    headers: HeaderMap,
    State(state): State<Arc<Mutex<AppState>>>,
) -> Result<(), FeedCreateError> {
    let feed = headers.get("HX-Prompt").ok_or(FeedCreateError::BadInput(
        "somehow the HX-Prompt header did not get included",
    ))?;

    let s = feed
        .to_str()
        .map_err(|_| FeedCreateError::BadInput("could not convert HX-Prompt value to str"))?;

    let feed_url =
        Url::parse(s).map_err(|_| FeedCreateError::BadInput("could not parse str as URL"))?;

    let state = state.lock().await;
    let mut conn = state.pool.acquire().await?;
    let http_client = state.http_client.clone();
    drop(state);

    let already_exists: Option<(bool,)> = sqlx::query_as(
        "
    select
        1
    from feeds
    where feed_link = ?",
    )
    .bind(feed_url.as_str())
    .fetch_optional(&mut *conn)
    .await?;

    if already_exists.is_some() {
        return Err(FeedCreateError::BadInput("Feed already exists"));
    }

    let response = http_client
        .get(feed_url.clone())
        .send()
        .await?
        .error_for_status()?;

    let body = response.bytes().await?;

    let feed = feed_rs::parser::parse(&*body)?;

    let mut tx = conn.begin().await?;

    #[derive(FromRow)]
    struct Feed {
        id: i64,
    }

    let Feed { id: feed_id } = sqlx::query_as(
        "
        insert into feeds (title, link, feed_link, feed_kind)
        values (?1, ?2, ?3, ?4)
        returning id",
    )
    .bind(&feed.title.as_ref().unwrap().content)
    .bind(&feed.links.first().unwrap().href)
    .bind(feed_url.as_str())
    .bind(match feed.feed_type {
        feed_rs::model::FeedType::Atom => "Atom",
        feed_rs::model::FeedType::JSON => "JSON",
        feed_rs::model::FeedType::RSS0 => "RSS",
        feed_rs::model::FeedType::RSS1 => "RSS",
        feed_rs::model::FeedType::RSS2 => "RSS",
    })
    .fetch_one(&mut *tx)
    .await?;

    for entry in &feed.entries {
        sqlx::query(
            "
            insert into entries (feed_id, title, author, pub_date, content, link)
            values (?1, ?2, ?3, ?4, ?5, ?6)
            ",
        )
        .bind(feed_id)
        .bind(entry.title.as_ref().map(|title| &title.content))
        .bind(entry.authors.first().map(|author| &author.name))
        .bind(entry.published)
        .bind(entry.content.as_ref().map(|content| &content.body))
        .bind(entry.links.first().map(|link| &link.href))
        .execute(&mut *tx)
        .await?;
    }

    tx.commit().await?;

    Ok(())
}

async fn feed_create(
    headers: HeaderMap,
    state: State<Arc<Mutex<AppState>>>,
) -> Result<impl IntoResponse, AppError> {
    match do_feed_create(headers, state).await {
        Ok(()) => {
            let mut headers = HeaderMap::new();
            headers.insert("HX-Location", "/".parse().unwrap());
            Ok((headers, "").into_response())
        }
        Err(e) => {
            let (status_code, error_message) = match e {
                FeedCreateError::BadInput(s) => (StatusCode::BAD_REQUEST, s.to_string()),
                FeedCreateError::NetworkError(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Unable to fetch remote feed: {e}"),
                ),
                FeedCreateError::FeedParseError(e) => (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    format!("Could not parse feed: {e}"),
                ),
                FeedCreateError::DatabaseError(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    format!("Database error: {e}"),
                ),
            };

            let mut headers = HeaderMap::new();

            headers.insert(
                "HX-Trigger",
                axum::Json(format!("{{\"feedCreateError\":\"{error_message}\"}}"))
                    .parse()
                    .unwrap(),
            );

            Ok((status_code, headers, "").into_response())
        }
    }
}

async fn static_handler(uri: Uri) -> impl IntoResponse {
    let mut path = uri.path().trim_start_matches('/').to_string();

    if path.starts_with("dist/") {
        path = path.replace("dist/", "");
    }

    StaticFile(path)
}

#[instrument(skip(conn))]
async fn initialize_db(conn: &mut sqlx::SqliteConnection) -> anyhow::Result<()> {
    let mut tx = conn.begin().await?;

    let (schema_version,): (u64,) = sqlx::query_as("PRAGMA user_version")
        .fetch_optional(&mut *tx)
        .await?
        .unwrap_or((0,));

    if schema_version == 0 {
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

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS entries_feed_id_and_pub_date_and_inserted_at_index
        ON entries (feed_id, pub_date, inserted_at)",
        )
        .execute(&mut *tx)
        .await?;
    }

    if schema_version <= 1 {
        tx.execute("PRAGMA user_version=2").await?;

        sqlx::query("ALTER TABLE feeds ADD COLUMN latest_etag TEXT")
            .execute(&mut *tx)
            .await?;
    }

    if schema_version <= 2 {
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

#[derive(Embed)]
#[folder = "dist/"]
struct Asset;

pub struct StaticFile<T>(pub T);

impl<T> IntoResponse for StaticFile<T>
where
    T: Into<String>,
{
    fn into_response(self) -> Response {
        let path = self.0.into();

        match Asset::get(path.as_str()) {
            Some(content) => {
                let mime = mime_guess::from_path(path).first_or_octet_stream();
                (
                    [(axum::http::header::CONTENT_TYPE, mime.as_ref())],
                    content.data,
                )
                    .into_response()
            }
            None => (StatusCode::NOT_FOUND, "404 Not Found").into_response(),
        }
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

    let router = Router::new()
        .route("/", get(feed_index))
        .route("/feeds", post(feed_create))
        .route("/feeds/{feed_id}", get(feed_show))
        .route("/entries/{entry_id}", get(entry_show).put(entry_update))
        .route("/dist/{*file}", get(static_handler))
        .with_state(state)
        .layer(tower_http::compression::CompressionLayer::new());

    #[cfg(debug_assertions)]
    let router = router.layer(tower_livereload::LiveReloadLayer::new());

    let listener = tokio::net::TcpListener::bind("localhost:4000").await?;

    axum::serve(listener, router).await?;

    Ok(())
}
