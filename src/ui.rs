use std::borrow::Cow;
use std::cell::Cell;
use std::collections::{hash_map::DefaultHasher, HashMap, HashSet};
use std::env;
use std::fs;
use std::hash::{Hash, Hasher};
use std::io::{self, Cursor, Read, Stdout, Write};
use std::path::Path;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, OnceLock,
};
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use crossbeam_channel::{unbounded, Receiver, Sender};
use crossterm::cursor::MoveTo;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, MouseEvent, MouseEventKind};
use crossterm::style::Print;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, window_size, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::ExecutableCommand;
use image::{self, ImageFormat};
use once_cell::sync::Lazy;
use percent_encoding::percent_decode_str;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};
use ratatui::widgets::{
    Block, Borders, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap,
};
use ratatui::{Frame, Terminal};
use reqwest::blocking::Client;
use semver::Version;
use unicode_width::UnicodeWidthStr;
use url::Url;

use base64::{engine::general_purpose, Engine as _};
use regex::{Captures, Regex};
use textwrap::{wrap, Options as WrapOptions};

use crate::auth;
use crate::config;
use crate::data::{CommentService, FeedService, InteractionService, SubredditService};
use crate::markdown;
use crate::media;
use crate::reddit;
use crate::session;
use crate::storage;
use crate::update;
use copypasta::{ClipboardContext, ClipboardProvider};

const MAX_IMAGE_COLS: i32 = 40;
const MAX_IMAGE_ROWS: i32 = 20;
const TARGET_PREVIEW_WIDTH_PX: i64 = 480;
const KITTY_CHUNK_SIZE: usize = 4096;
const MEDIA_INDENT: u16 = 0;

// TODO video preview
const COLOR_BG: Color = Color::Rgb(30, 30, 46);
const COLOR_PANEL_BG: Color = Color::Rgb(24, 24, 36);
const COLOR_PANEL_FOCUSED_BG: Color = Color::Rgb(49, 50, 68);
const COLOR_PANEL_SELECTED_BG: Color = Color::Rgb(69, 71, 90);
const COLOR_BORDER_IDLE: Color = Color::Rgb(49, 50, 68);
const COLOR_BORDER_FOCUSED: Color = Color::Rgb(137, 180, 250);
const COLOR_TEXT_PRIMARY: Color = Color::Rgb(205, 214, 244);
const COLOR_TEXT_SECONDARY: Color = Color::Rgb(166, 173, 200);
const COLOR_ACCENT: Color = Color::Rgb(137, 180, 250);
const COLOR_SUCCESS: Color = Color::Rgb(166, 227, 161);
const COLOR_ERROR: Color = Color::Rgb(243, 139, 168);

const PROJECT_LINK_URL: &str = "https://github.com/ck-zhang/reddix";
const SUPPORT_LINK_URL: &str = "https://ko-fi.com/ckzhang";
const UPDATE_CHECK_DISABLE_ENV: &str = "REDDIX_SKIP_UPDATE_CHECK";
const REDDIX_COMMUNITY: &str = "ReddixTUI";
const REDDIX_COMMUNITY_DISPLAY: &str = "r/ReddixTUI";
const COMMENT_DEPTH_COLORS: [Color; 6] = [
    Color::Rgb(250, 179, 135),
    Color::Rgb(166, 227, 161),
    Color::Rgb(203, 166, 247),
    Color::Rgb(245, 194, 231),
    Color::Rgb(137, 220, 235),
    Color::Rgb(249, 226, 175),
];

fn comment_depth_color(depth: usize) -> Color {
    COMMENT_DEPTH_COLORS[depth % COMMENT_DEPTH_COLORS.len()]
}

fn vote_from_likes(likes: Option<bool>) -> i32 {
    match likes {
        Some(true) => 1,
        Some(false) => -1,
        None => 0,
    }
}

fn likes_from_vote(vote: i32) -> Option<bool> {
    match vote {
        1 => Some(true),
        -1 => Some(false),
        _ => None,
    }
}

fn toggle_vote_value(old: i32, requested: i32) -> i32 {
    if old == requested {
        0
    } else {
        requested
    }
}

const NAV_SORTS: [reddit::SortOption; 5] = [
    reddit::SortOption::Hot,
    reddit::SortOption::Best,
    reddit::SortOption::New,
    reddit::SortOption::Top,
    reddit::SortOption::Rising,
];
const FEED_CACHE_TTL: Duration = Duration::from_secs(45);
const COMMENT_CACHE_TTL: Duration = Duration::from_secs(120);
const FEED_CACHE_MAX: usize = 16;
const POST_PRELOAD_THRESHOLD: usize = 5;
const COMMENT_CACHE_MAX: usize = 64;
const SPINNER_FRAMES: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
const POST_LOADING_HEADER_HEIGHT: usize = 2;
const UPDATE_BANNER_HEIGHT: usize = 1;
const ICON_UPVOTES: &str = "";
const ICON_COMMENTS: &str = "";
const ICON_SUBREDDIT: &str = "";
const ICON_USER: &str = "";

static HTTP_CLIENT: Lazy<Client> = Lazy::new(|| {
    Client::builder()
        .timeout(Duration::from_secs(10))
        .user_agent("reddix/0.1 (kitty-preview)")
        .build()
        .expect("create http client")
});

#[derive(Clone)]
pub struct PostPreview {
    pub title: String,
    pub body: String,
    pub post: reddit::Post,
    pub links: Vec<LinkEntry>,
}

#[derive(Clone)]
pub struct LinkEntry {
    pub label: String,
    pub url: String,
}

impl LinkEntry {
    fn new<L: Into<String>, U: Into<String>>(label: L, url: U) -> Self {
        Self {
            label: label.into(),
            url: url.into(),
        }
    }
}

#[derive(Clone)]
struct MediaPreview {
    placeholder: Text<'static>,
    kitty: Option<KittyImage>,
}

impl MediaPreview {
    fn placeholder(&self) -> &Text<'static> {
        &self.placeholder
    }

    fn kitty_mut(&mut self) -> Option<&mut KittyImage> {
        self.kitty.as_mut()
    }

    fn has_kitty(&self) -> bool {
        self.kitty.is_some()
    }
}

#[derive(Clone)]
struct KittyImage {
    id: u32,
    cols: i32,
    rows: i32,
    transmit_chunks: Vec<String>,
    transmitted: bool,
    wrap_tmux: bool,
}

impl KittyImage {
    fn ensure_transmitted<W: Write>(&mut self, writer: &mut W) -> io::Result<()> {
        if self.transmitted {
            return Ok(());
        }
        for chunk in &self.transmit_chunks {
            writer.write_all(chunk.as_bytes())?;
        }
        writer.flush()?;
        self.transmitted = true;
        Ok(())
    }

    fn placement_sequence(&self) -> String {
        let base = format!(
            "\x1b_Ga=p,q=2,C=1,i={},c={},r={};\x1b\\",
            self.id, self.cols, self.rows
        );
        if self.wrap_tmux {
            format!("\x1bPtmux;\x1b{base}\x1b\\")
        } else {
            base
        }
    }

    fn delete_sequence(&self) -> String {
        Self::delete_sequence_for(self.id, self.wrap_tmux)
    }

    fn delete_sequence_for(id: u32, wrap_tmux: bool) -> String {
        let base = format!("\x1b_Ga=d,q=2,i={id};\x1b\\");
        if wrap_tmux {
            format!("\x1bPtmux;\x1b{}\x1b\\", base)
        } else {
            base
        }
    }
}

#[derive(Clone, Copy, Default)]
struct MediaLayout {
    line_offset: usize,
    indent: u16,
}

#[derive(Clone, Copy)]
struct CellMetrics {
    width: f64,
    height: f64,
}

fn terminal_cell_metrics() -> CellMetrics {
    static METRICS: OnceLock<CellMetrics> = OnceLock::new();
    *METRICS.get_or_init(|| {
        window_size().ok().map_or(
            CellMetrics {
                width: 1.0,
                height: 1.0,
            },
            |size| {
                let columns = size.columns.max(1) as f64;
                let rows = size.rows.max(1) as f64;
                let width = if size.width > 0 && columns > 0.0 {
                    f64::from(size.width) / columns
                } else {
                    1.0
                };
                let height = if size.height > 0 && rows > 0.0 {
                    f64::from(size.height) / rows
                } else {
                    1.0
                };
                CellMetrics { width, height }
            },
        )
    })
}

fn kitty_debug_enabled() -> bool {
    static FLAG: OnceLock<bool> = OnceLock::new();
    *FLAG.get_or_init(|| env_truthy("REDDIX_DEBUG_KITTY"))
}

struct ActiveKitty {
    post_name: String,
    image_id: u32,
    wrap_tmux: bool,
}

struct NumericJump {
    value: usize,
    last_input: Instant,
}

fn collect_comments(
    listing: &reddit::Listing<reddit::Comment>,
    depth: usize,
    entries: &mut Vec<CommentEntry>,
) -> usize {
    let mut total = 0;
    for thing in &listing.children {
        if thing.kind == "more" {
            continue;
        }
        let comment = &thing.data;
        if comment.body.trim().is_empty() {
            continue;
        }
        let index = entries.len();
        let (clean_body, found_links) = scrub_links(&comment.body);
        let author_label = if comment.author.trim().is_empty() {
            "[deleted]".to_string()
        } else {
            let author = comment.author.as_str();
            format!("u/{author}")
        };
        let mut link_entries = Vec::new();
        for (idx, url) in found_links.into_iter().enumerate() {
            let number = idx + 1;
            let label = format!("Comment link {number} ({author_label})");
            link_entries.push(LinkEntry::new(label, url));
        }
        entries.push(CommentEntry {
            name: comment.name.clone(),
            author: comment.author.clone(),
            body: clean_body,
            score: comment.score,
            likes: comment.likes,
            depth,
            descendant_count: 0,
            links: link_entries,
        });
        let child_count = comment
            .replies
            .as_ref()
            .map(|replies| collect_comments(replies, depth + 1, entries))
            .unwrap_or(0);
        entries[index].descendant_count = child_count;
        total += 1 + child_count;
    }
    total
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let percent_x = percent_x.min(100);
    let percent_y = percent_y.min(100);
    let horizontal = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage(100 - percent_x - (100 - percent_x) / 2),
        ])
        .split(area);
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage(100 - percent_y - (100 - percent_y) / 2),
        ])
        .split(horizontal[1]);
    vertical[1]
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Pane {
    Navigation,
    Posts,
    Content,
    Comments,
}

impl Pane {
    fn title(self) -> &'static str {
        match self {
            Pane::Navigation => "Navigation",
            Pane::Posts => "Posts",
            Pane::Content => "Content",
            Pane::Comments => "Comments",
        }
    }

    fn next(self) -> Self {
        match self {
            Pane::Navigation => Pane::Posts,
            Pane::Posts => Pane::Content,
            Pane::Content => Pane::Comments,
            Pane::Comments => Pane::Comments,
        }
    }

    fn previous(self) -> Self {
        match self {
            Pane::Navigation => Pane::Navigation,
            Pane::Posts => Pane::Navigation,
            Pane::Content => Pane::Posts,
            Pane::Comments => Pane::Content,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum MenuField {
    #[default]
    ClientId,
    ClientSecret,
    UserAgent,
    Save,
    CopyLink,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum MenuScreen {
    Accounts,
    Credentials,
}

#[derive(Clone)]
struct MenuAccountEntry {
    id: i64,
    display: String,
    is_active: bool,
}

#[derive(Default, Clone)]
struct JoinState {
    pending: bool,
    joined: bool,
    last_error: Option<String>,
}

struct MenuAccountPositions {
    add: usize,
    join: usize,
    github: usize,
    support: usize,
    total: usize,
}

impl JoinState {
    fn mark_pending(&mut self) {
        self.pending = true;
        self.last_error = None;
    }

    fn mark_success(&mut self) {
        self.pending = false;
        self.joined = true;
        self.last_error = None;
    }

    fn mark_error(&mut self, message: String) {
        self.pending = false;
        self.joined = false;
        self.last_error = Some(message);
    }
}

impl MenuField {
    fn next(self, has_copy: bool) -> Self {
        match self {
            MenuField::ClientId => MenuField::ClientSecret,
            MenuField::ClientSecret => MenuField::UserAgent,
            MenuField::UserAgent => MenuField::Save,
            MenuField::Save => {
                if has_copy {
                    MenuField::CopyLink
                } else {
                    MenuField::ClientId
                }
            }
            MenuField::CopyLink => MenuField::ClientId,
        }
    }

    fn previous(self, has_copy: bool) -> Self {
        match self {
            MenuField::ClientId => {
                if has_copy {
                    MenuField::CopyLink
                } else {
                    MenuField::Save
                }
            }
            MenuField::ClientSecret => MenuField::ClientId,
            MenuField::UserAgent => MenuField::ClientSecret,
            MenuField::Save => MenuField::UserAgent,
            MenuField::CopyLink => MenuField::Save,
        }
    }

    fn title(self) -> &'static str {
        match self {
            MenuField::ClientId => "Reddit Client ID",
            MenuField::ClientSecret => "Reddit Client Secret",
            MenuField::UserAgent => "User Agent",
            MenuField::Save => "Save & Close",
            MenuField::CopyLink => "Copy Authorization Link",
        }
    }
}

#[derive(Default)]
struct MenuForm {
    active: MenuField,
    client_id: String,
    client_secret: String,
    user_agent: String,
    status: Option<String>,
    auth_url: Option<String>,
    auth_pending: bool,
}

impl MenuForm {
    fn reset_status(&mut self) {
        self.status = None;
    }

    fn set_status<S: Into<String>>(&mut self, message: S) {
        self.status = Some(message.into());
    }

    fn focus(&mut self, field: MenuField) {
        if !self.has_auth_link() && matches!(field, MenuField::CopyLink) {
            self.active = MenuField::Save;
        } else {
            self.active = field;
        }
    }

    fn next(&mut self) {
        let has_copy = self.has_auth_link();
        self.active = self.active.next(has_copy);
    }

    fn previous(&mut self) {
        let has_copy = self.has_auth_link();
        self.active = self.active.previous(has_copy);
    }

    fn set_values(&mut self, client_id: String, client_secret: String, user_agent: String) {
        self.client_id = client_id;
        self.client_secret = client_secret;
        self.user_agent = user_agent;
    }

    fn active_value_mut(&mut self) -> Option<&mut String> {
        match self.active {
            MenuField::ClientId => Some(&mut self.client_id),
            MenuField::ClientSecret => Some(&mut self.client_secret),
            MenuField::UserAgent => Some(&mut self.user_agent),
            MenuField::Save | MenuField::CopyLink => None,
        }
    }

    fn insert_char(&mut self, ch: char) {
        if let Some(value) = self.active_value_mut() {
            value.push(ch);
        }
        self.reset_status();
    }

    fn backspace(&mut self) {
        if let Some(value) = self.active_value_mut() {
            value.pop();
        }
        self.reset_status();
    }

    fn clear_active(&mut self) {
        if let Some(value) = self.active_value_mut() {
            value.clear();
        }
        self.reset_status();
    }

    fn trimmed_values(&self) -> (String, String, String) {
        (
            self.client_id.trim().to_string(),
            self.client_secret.trim().to_string(),
            self.user_agent.trim().to_string(),
        )
    }

    fn display_value(&self, field: MenuField) -> String {
        let raw = match field {
            MenuField::ClientId => &self.client_id,
            MenuField::ClientSecret => &self.client_secret,
            MenuField::UserAgent => &self.user_agent,
            MenuField::Save | MenuField::CopyLink => return String::new(),
        };
        if raw.is_empty() {
            return "(not set)".to_string();
        }
        if matches!(field, MenuField::ClientSecret) {
            return "*".repeat(raw.chars().count().max(1));
        }
        raw.clone()
    }

    fn authorization_started(&mut self, url: String) {
        self.auth_url = Some(url);
        self.auth_pending = true;
        self.focus(MenuField::CopyLink);
    }

    fn authorization_complete(&mut self) {
        self.auth_pending = false;
        self.auth_url = None;
        if matches!(self.active, MenuField::CopyLink) {
            self.active = MenuField::Save;
        }
    }

    fn has_auth_link(&self) -> bool {
        self.auth_url.is_some()
    }

    fn auth_link(&self) -> Option<&str> {
        self.auth_url.as_deref()
    }
}

#[derive(Clone)]
struct CommentEntry {
    name: String,
    author: String,
    body: String,
    score: i64,
    likes: Option<bool>,
    depth: usize,
    descendant_count: usize,
    links: Vec<LinkEntry>,
}

#[derive(Clone)]
struct PostRowData {
    identity: Vec<Line<'static>>,
    title: Vec<Line<'static>>,
    metrics: Vec<Line<'static>>,
}

#[derive(Clone)]
struct PostRowInput {
    name: String,
    title: String,
    subreddit: String,
    author: String,
    score: i64,
    comments: i64,
    vote: i32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum NavMode {
    Sorts,
    Subreddits,
}

struct PendingPosts {
    request_id: u64,
    cancel_flag: Arc<AtomicBool>,
    mode: LoadMode,
}

struct PendingComments {
    request_id: u64,
    post_name: String,
    cancel_flag: Arc<AtomicBool>,
}

struct PendingSubreddits {
    request_id: u64,
}

struct PendingPostRows {
    request_id: u64,
    width: usize,
}

struct PendingContent {
    request_id: u64,
    post_name: String,
    cancel_flag: Arc<AtomicBool>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum LoadMode {
    Replace,
    Append,
}

#[derive(Clone)]
enum VoteTarget {
    Post { fullname: String },
    Comment { fullname: String },
}

enum AsyncResponse {
    Posts {
        request_id: u64,
        target: String,
        sort: reddit::SortOption,
        result: Result<PostBatch>,
    },
    PostRows {
        request_id: u64,
        width: usize,
        rows: Vec<(String, PostRowData)>,
    },
    Comments {
        request_id: u64,
        post_name: String,
        result: Result<Vec<CommentEntry>>,
    },
    Content {
        request_id: u64,
        post_name: String,
        rendered: Text<'static>,
    },
    Subreddits {
        request_id: u64,
        result: Result<Vec<String>>,
    },
    Media {
        post_name: String,
        result: Result<Option<MediaPreview>>,
    },
    Login {
        result: Result<String>,
    },
    Update {
        result: Result<Option<update::UpdateInfo>>,
    },
    JoinStatus {
        account_id: i64,
        result: Result<bool>,
    },
    JoinCommunity {
        account_id: i64,
        result: Result<()>,
    },
    VoteResult {
        target: VoteTarget,
        requested: i32,
        previous: i32,
        error: Option<String>,
    },
}

fn comment_lines(
    comment: &CommentEntry,
    width: usize,
    indicator: &str,
    meta_style: Style,
    body_style: Style,
    collapsed: bool,
) -> Vec<Line<'static>> {
    let indent_units = "  ".repeat(comment.depth);
    let indicator_prefix = format!("{indent_units}{indicator} ");
    let spacer = " ".repeat(indicator.chars().count());
    let rest_prefix = format!("{indent_units}{spacer} ");
    let body_prefix = format!("{indent_units}{spacer}  ");

    let author = if comment.author.trim().is_empty() {
        "[deleted]"
    } else {
        comment.author.as_str()
    };

    let vote_marker = match comment.likes {
        Some(true) => "▲",
        Some(false) => "▼",
        None => "·",
    };

    let score = comment.score;
    let mut header = format!("{vote_marker} u/{author} · {score} points");
    if collapsed {
        let hidden = comment.descendant_count;
        if hidden > 0 {
            let suffix = if hidden == 1 { "reply" } else { "replies" };
            header.push_str(&format!(" · {hidden} hidden {suffix}"));
        }
    }

    let mut lines = wrap_with_prefixes(
        &header,
        width,
        indicator_prefix.as_str(),
        rest_prefix.as_str(),
        meta_style,
    );

    if comment.body.trim().is_empty() {
        lines.extend(wrap_with_prefix(
            "(no comment body)",
            width,
            body_prefix.as_str(),
            body_style,
        ));
        return lines;
    }

    for raw_line in comment.body.lines() {
        if raw_line.trim().is_empty() {
            lines.push(Line::from(Span::styled(String::new(), body_style)));
            continue;
        }
        lines.extend(wrap_with_prefix(
            raw_line.trim(),
            width,
            body_prefix.as_str(),
            body_style,
        ));
    }

    lines
}

fn is_front_page(name: &str) -> bool {
    let normalized = name.trim().trim_start_matches("r/").trim_start_matches('/');
    normalized.eq_ignore_ascii_case("frontpage")
        || normalized.eq_ignore_ascii_case("home")
        || normalized.is_empty()
}

fn normalize_subreddit_name(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_slashes = trimmed.trim_start_matches('/');
    let rest = if let Some(stripped) = without_slashes
        .strip_prefix("r/")
        .or_else(|| without_slashes.strip_prefix("R/"))
    {
        stripped.trim_start_matches('/')
    } else {
        without_slashes.trim_start_matches('/')
    };
    let rest = rest.trim();
    if rest.is_empty() {
        "r/frontpage".to_string()
    } else {
        format!("r/{}", rest)
    }
}

fn ensure_core_subreddits(subreddits: &mut Vec<String>) {
    let mut combined = vec![
        "r/frontpage".to_string(),
        "r/all".to_string(),
        "r/popular".to_string(),
    ];

    for name in subreddits.drain(..) {
        if !combined
            .iter()
            .any(|existing| existing.eq_ignore_ascii_case(name.as_str()))
        {
            combined.push(name);
        }
    }

    *subreddits = combined;
}

fn fallback_feed_target(current: &str) -> Option<&'static str> {
    let normalized = normalize_subreddit_name(current);
    if is_front_page(&normalized) {
        Some("r/popular")
    } else if normalized.eq_ignore_ascii_case("r/popular") {
        Some("r/all")
    } else {
        None
    }
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
enum CacheScope {
    Anonymous,
    Account(i64),
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct FeedCacheKey {
    target: String,
    sort: reddit::SortOption,
}

impl FeedCacheKey {
    fn new(target: &str, sort: reddit::SortOption) -> Self {
        Self {
            target: target.trim().to_ascii_lowercase(),
            sort,
        }
    }
}

struct FeedCacheEntry {
    batch: PostBatch,
    fetched_at: Instant,
    scope: CacheScope,
}

struct CommentCacheEntry {
    comments: Vec<CommentEntry>,
    fetched_at: Instant,
    scope: CacheScope,
}

struct Spinner {
    index: usize,
    last_tick: Instant,
}

#[derive(Clone)]
struct PostBatch {
    posts: Vec<PostPreview>,
    after: Option<String>,
}

impl Spinner {
    fn new() -> Self {
        Self {
            index: 0,
            last_tick: Instant::now(),
        }
    }

    fn frame(&self) -> &'static str {
        SPINNER_FRAMES[self.index % SPINNER_FRAMES.len()]
    }

    fn advance(&mut self) -> bool {
        let now = Instant::now();
        if now.duration_since(self.last_tick) >= Duration::from_millis(120) {
            self.index = (self.index + 1) % SPINNER_FRAMES.len();
            self.last_tick = now;
            true
        } else {
            false
        }
    }

    fn reset(&mut self) {
        self.index = 0;
        self.last_tick = Instant::now();
    }
}

fn make_preview(post: reddit::Post) -> PostPreview {
    let mut body = String::new();
    let mut links: Vec<LinkEntry> = Vec::new();
    let title = post.title.trim();
    body.push_str(&format!(
        "# {}\n\n",
        if title.is_empty() { "Untitled" } else { title }
    ));

    let trimmed_self = post.selftext.trim();
    if !trimmed_self.is_empty() {
        let (clean_self, found_links) = scrub_links(trimmed_self);
        body.push_str(&clean_self);
        body.push_str("\n\n");
        for (index, url) in found_links.into_iter().enumerate() {
            links.push(LinkEntry::new(format!("Post body link {}", index + 1), url));
        }
    } else {
        let url = post.url.trim();
        if !url.is_empty() {
            links.push(LinkEntry::new("External link", url.to_string()));
        }
        if select_preview_source(&post).is_some() {
            // image preview will render asynchronously; no placeholder text needed
        } else if post.post_hint.eq_ignore_ascii_case("hosted:video")
            || post.post_hint.eq_ignore_ascii_case("rich:video")
        {
            body.push_str("_No inline video preview available yet._\n\n");
        } else {
            body.push_str("_No preview available for this post._\n\n");
        }
    }

    body.push_str("---\n\n");

    let meta_lines: Vec<String> = vec![
        format!("**Subreddit:** {}", post.subreddit),
        format!("**Author:** u/{}", post.author),
        format!("**Score:** {}", post.score),
        format!("**Comments:** {}", post.num_comments),
    ];

    let url = post.url.trim();
    if !url.is_empty() && !links.iter().any(|entry| entry.url == url) {
        links.push(LinkEntry::new("External link", url.to_string()));
    }

    let permalink = post.permalink.trim();
    if !permalink.is_empty() {
        let thread_url = format!("https://reddit.com{}", permalink);
        if !links.iter().any(|entry| entry.url == thread_url) {
            links.push(LinkEntry::new("Reddit thread", thread_url));
        }
    }

    for line in meta_lines {
        body.push_str(&format!("- {}\n", line));
    }

    body.push('\n');

    PostPreview {
        title: post.title.clone(),
        body,
        post,
        links,
    }
}

fn content_from_post(post: &PostPreview) -> String {
    post.body.clone()
}

fn load_media_preview(
    post: &reddit::Post,
    cancel_flag: &AtomicBool,
    media_handle: Option<media::Handle>,
) -> Result<Option<MediaPreview>> {
    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }

    // TODO gallery support

    let source = match select_preview_source(post) {
        Some(src) => src,
        None => return Ok(None),
    };

    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }

    let url = source.url.clone();
    if !is_supported_preview_url(&url) {
        let fallback = indent_media_preview(&format!(
            "[preview omitted: unsupported media — {}]",
            image_label(&url)
        ));
        return Ok(Some(MediaPreview {
            placeholder: text_from_string(fallback),
            kitty: None,
        }));
    }

    if !is_kitty_terminal() {
        let fallback = indent_media_preview(&format!(
            "[inline image preview disabled; set REDDIX_FORCE_KITTY=1 to re-enable if your terminal supports the Kitty protocol. Image: {}]",
            image_label(&url)
        ));
        return Ok(Some(MediaPreview {
            placeholder: text_from_string(fallback),
            kitty: None,
        }));
    }

    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }

    let bytes = if let Some(handle) = media_handle {
        match fetch_cached_media_bytes(handle, &url, source.width, source.height, cancel_flag) {
            Ok(Some(bytes)) => bytes,
            Ok(None) => return Ok(None),
            Err(_) => fetch_image_bytes(&url)
                .with_context(|| format!("download preview image {}", url))?,
        }
    } else {
        fetch_image_bytes(&url).with_context(|| format!("download preview image {}", url))?
    };
    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }
    if bytes.is_empty() {
        bail!("preview image empty");
    }

    let (cols, rows) =
        clamp_dimensions(source.width, source.height, MAX_IMAGE_COLS, MAX_IMAGE_ROWS);
    let label = image_label(&url);
    let kitty = kitty_transmit_inline(&bytes, cols, rows, kitty_image_id(&post.name, &url))?;
    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }
    let placeholder = kitty_placeholder_text(cols, rows, MEDIA_INDENT, &label);
    Ok(Some(MediaPreview {
        placeholder,
        kitty: Some(kitty),
    }))
}

fn fetch_cached_media_bytes(
    handle: media::Handle,
    url: &str,
    width: i64,
    height: i64,
    cancel_flag: &AtomicBool,
) -> Result<Option<Vec<u8>>> {
    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }

    let request = media::Request {
        url: url.to_string(),
        width: (width > 0).then_some(width),
        height: (height > 0).then_some(height),
        ..Default::default()
    };

    let rx = handle.enqueue(request);
    let result = rx
        .recv()
        .map_err(|err| anyhow!("media: failed to receive cache result: {}", err))?;

    if cancel_flag.load(Ordering::SeqCst) {
        return Ok(None);
    }

    if let Some(entry) = result.entry {
        let path = Path::new(&entry.file_path);
        let bytes =
            fs::read(path).with_context(|| format!("read cached media {}", path.display()))?;
        Ok(Some(bytes))
    } else if let Some(err) = result.error {
        Err(err)
    } else {
        bail!("media: cache returned empty result")
    }
}

fn kitty_placeholder_text(cols: i32, rows: i32, indent: u16, label: &str) -> Text<'static> {
    let row_count = rows.max(1) as usize;
    let indent_width = indent as usize;
    let indent_str = " ".repeat(indent_width);
    let column_span = " ".repeat(cols.max(1) as usize);
    let row_line = format!("{}{}", indent_str, column_span);
    let mut lines: Vec<Line<'static>> = Vec::with_capacity(row_count + 1);
    for _ in 0..row_count {
        lines.push(Line::from(row_line.clone()));
    }
    let label_line = format!("{}[image: {}]", indent_str, label);
    lines.push(Line::from(Span::styled(
        label_line,
        Style::default().fg(COLOR_TEXT_SECONDARY),
    )));
    text_with_lines(lines)
}

fn kitty_image_id(post_name: &str, url: &str) -> u32 {
    let mut hasher = DefaultHasher::new();
    post_name.hash(&mut hasher);
    url.hash(&mut hasher);
    (hasher.finish() & 0xFFFF_FFFF) as u32
}

fn select_preview_source(post: &reddit::Post) -> Option<reddit::PreviewSource> {
    post.preview
        .images
        .iter()
        .find(|image| {
            !image.source.url.trim().is_empty()
                || image
                    .resolutions
                    .iter()
                    .any(|res| !res.url.trim().is_empty())
        })
        .and_then(|image| {
            let mut larger: Option<(i64, reddit::PreviewSource)> = None;
            let mut smaller: Option<(i64, reddit::PreviewSource)> = None;

            for candidate in image
                .resolutions
                .iter()
                .chain(std::iter::once(&image.source))
            {
                let sanitized = sanitize_preview_url(&candidate.url);
                if sanitized.is_empty() {
                    continue;
                }
                let mut candidate = candidate.clone();
                candidate.url = sanitized;
                let width = if candidate.width > 0 {
                    candidate.width
                } else {
                    TARGET_PREVIEW_WIDTH_PX
                };

                if width >= TARGET_PREVIEW_WIDTH_PX {
                    let replace = larger
                        .as_ref()
                        .map(|(existing_width, _)| width < *existing_width)
                        .unwrap_or(true);
                    if replace {
                        larger = Some((width, candidate.clone()));
                    }
                } else {
                    let replace = smaller
                        .as_ref()
                        .map(|(existing_width, _)| width > *existing_width)
                        .unwrap_or(true);
                    if replace {
                        smaller = Some((width, candidate.clone()));
                    }
                }
            }

            larger.or(smaller).map(|(_, candidate)| candidate)
        })
}

fn sanitize_preview_url(raw: &str) -> String {
    raw.replace("&amp;", "&")
}

fn is_supported_preview_url(url: &str) -> bool {
    if url.trim().is_empty() {
        return false;
    }

    let lowered = url.to_ascii_lowercase();
    if lowered.contains("format=mp4")
        || lowered.contains("format=gif")
        || lowered.contains("format=gifv")
        || lowered.contains("format=webm")
    {
        return false;
    }

    match Url::parse(url) {
        Ok(parsed) => {
            if let Some(ext) = parsed
                .path()
                .rsplit('.')
                .next()
                .map(|value| value.to_ascii_lowercase())
            {
                match ext.as_str() {
                    "jpg" | "jpeg" | "png" | "webp" | "jpe" => {}
                    "gif" | "gifv" | "mp4" | "webm" | "mkv" => return false,
                    _ => {}
                }
            }

            for (key, value) in parsed.query_pairs() {
                if key.eq_ignore_ascii_case("format") {
                    let value = value.to_ascii_lowercase();
                    if matches!(value.as_str(), "mp4" | "gif" | "gifv" | "webm" | "mkv") {
                        return false;
                    }
                }
            }
            true
        }
        Err(_) => {
            !lowered.ends_with(".mp4")
                && !lowered.ends_with(".gif")
                && !lowered.ends_with(".gifv")
                && !lowered.ends_with(".webm")
                && !lowered.ends_with(".mkv")
        }
    }
}

fn fetch_image_bytes(url: &str) -> Result<Vec<u8>> {
    let response = HTTP_CLIENT
        .get(url)
        .send()
        .with_context(|| format!("request preview {}", url))?;
    if !response.status().is_success() {
        bail!("preview request returned status {}", response.status());
    }
    let mut reader = response;
    let mut bytes = Vec::with_capacity(128 * 1024);
    reader
        .read_to_end(&mut bytes)
        .with_context(|| format!("read preview body {}", url))?;
    Ok(bytes)
}

fn encode_png_for_kitty(bytes: &[u8]) -> Result<Cow<'_, [u8]>> {
    if bytes.is_empty() {
        bail!("preview image had no bytes");
    }

    if matches!(image::guess_format(bytes), Ok(ImageFormat::Png)) {
        return Ok(Cow::Borrowed(bytes));
    }

    let image = image::load_from_memory(bytes).context("decode preview image")?;
    let mut png_bytes = Vec::new();
    image
        .write_to(&mut Cursor::new(&mut png_bytes), ImageFormat::Png)
        .context("encode preview as png")?;
    Ok(Cow::Owned(png_bytes))
}

fn tmux_passthrough_enabled() -> bool {
    env::var("TMUX").map(|v| !v.is_empty()).unwrap_or(false)
}

fn kitty_transmit_inline(bytes: &[u8], cols: i32, rows: i32, image_id: u32) -> Result<KittyImage> {
    if bytes.is_empty() {
        bail!("no image data provided");
    }

    let png_data = encode_png_for_kitty(bytes)?;

    let cols = cols.max(1);
    let rows = rows.max(1);
    let encoded = general_purpose::STANDARD.encode(png_data.as_ref());
    if encoded.is_empty() {
        bail!("failed to encode image preview");
    }

    let wrap_tmux = tmux_passthrough_enabled();
    let prefix = if wrap_tmux { "\x1bPtmux;\x1b" } else { "" };
    let suffix = if wrap_tmux { "\x1b\\" } else { "" };

    let mut chunks: Vec<String> = Vec::new();
    let mut offset = 0;
    while offset < encoded.len() {
        let end = usize::min(offset + KITTY_CHUNK_SIZE, encoded.len());
        let more = if end < encoded.len() { 1 } else { 0 };
        let mut out = String::new();
        if wrap_tmux {
            out.push_str(prefix);
        }
        if offset == 0 {
            out.push_str(&format!("\x1b_Ga=t,q=2,i={},f=100,m={more};", image_id));
        } else {
            out.push_str(&format!("\x1b_Ga=t,q=2,i={},m={more};", image_id));
        }
        out.push_str(&encoded[offset..end]);
        out.push_str("\x1b\\");
        if wrap_tmux {
            out.push_str(suffix);
        }
        chunks.push(out);
        offset = end;
    }

    Ok(KittyImage {
        id: image_id,
        cols,
        rows,
        transmit_chunks: chunks,
        transmitted: false,
        wrap_tmux,
    })
}

fn indent_media_preview(preview: &str) -> String {
    let text = preview.trim_start_matches('\n').to_string();
    if text.is_empty() {
        return text;
    }
    let indent = " ".repeat(MEDIA_INDENT as usize);
    if indent.is_empty() {
        return text;
    }
    let mut lines: Vec<String> = text.split('\n').map(|line| line.to_string()).collect();
    for line in &mut lines {
        if line.starts_with(&indent) || line.starts_with('\u{1b}') {
            continue;
        }
        if line.is_empty() {
            *line = indent.clone();
        } else {
            line.insert_str(0, indent.as_str());
        }
    }
    lines.join("\n")
}

fn text_from_string(preview: String) -> Text<'static> {
    let lines = preview
        .split('\n')
        .map(|line| Line::from(Span::raw(line.to_string())))
        .collect();
    text_with_lines(lines)
}

fn text_with_lines(lines: Vec<Line<'static>>) -> Text<'static> {
    Text {
        lines,
        alignment: Some(Alignment::Left),
        style: Style::default(),
    }
}

fn wrap_with_prefixes(
    text: &str,
    width: usize,
    first_prefix: &str,
    rest_prefix: &str,
    style: Style,
) -> Vec<Line<'static>> {
    if text.trim().is_empty() {
        return vec![Line::from(Span::styled(String::new(), style))];
    }

    if width == 0 {
        let mut line = String::with_capacity(first_prefix.len() + text.len());
        line.push_str(first_prefix);
        line.push_str(text);
        return vec![Line::from(Span::styled(line, style))];
    }

    let min_width = first_prefix
        .chars()
        .count()
        .max(rest_prefix.chars().count())
        .saturating_add(1);
    let wrap_width = width.max(min_width);
    let options = WrapOptions::new(wrap_width)
        .break_words(false)
        .initial_indent(first_prefix)
        .subsequent_indent(rest_prefix);

    wrap(text, options)
        .into_iter()
        .map(|cow| Line::from(Span::styled(cow.into_owned(), style)))
        .collect()
}

fn wrap_plain(text: &str, width: usize, style: Style) -> Vec<Line<'static>> {
    wrap_with_prefixes(text, width, "", "", style)
}

fn scrub_links(text: &str) -> (String, Vec<String>) {
    static MARKDOWN_LINK_RE: Lazy<Regex> = Lazy::new(|| {
        Regex::new(r"(?i)\[([^\]]+)\]\((https?://[^\s)]+)\)").expect("valid markdown link regex")
    });
    static BARE_URL_RE: Lazy<Regex> =
        Lazy::new(|| Regex::new(r"(?i)https?://[^\s)]+").expect("valid bare url regex"));

    if text.trim().is_empty() {
        return (text.to_string(), Vec::new());
    }

    let mut seen = HashSet::new();
    let mut links: Vec<String> = Vec::new();

    let intermediate = MARKDOWN_LINK_RE
        .replace_all(text, |caps: &Captures| {
            let url = caps[2].to_string();
            if seen.insert(url.clone()) {
                links.push(url);
            }
            caps[1].to_string()
        })
        .to_string();

    let sanitized = BARE_URL_RE
        .replace_all(&intermediate, |caps: &Captures| {
            let url = caps[0].to_string();
            if seen.insert(url.clone()) {
                links.push(url);
            }
            "[link]".to_string()
        })
        .to_string();

    (sanitized, links)
}

fn pad_lines_to_width(lines: &mut [Line<'static>], width: u16) {
    let width = width as usize;
    if width == 0 {
        return;
    }

    for line in lines {
        let mut current_width = 0usize;
        for span in &line.spans {
            current_width =
                current_width.saturating_add(UnicodeWidthStr::width(span.content.as_ref()));
        }
        if current_width >= width {
            continue;
        }
        let pad_style = line.spans.last().map(|span| span.style).unwrap_or_default();
        let padding = " ".repeat(width - current_width);
        line.spans.push(Span::styled(padding, pad_style));
    }
}

fn line_is_blank(line: &Line<'_>) -> bool {
    line.spans.is_empty()
        || line
            .spans
            .iter()
            .all(|span| span.content.as_ref().trim().is_empty())
}

fn line_visual_height(line: &Line<'_>, width: u16) -> usize {
    if width == 0 {
        return 0;
    }
    let width = width as usize;
    let content_width: usize = line
        .spans
        .iter()
        .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
        .sum();
    if content_width == 0 {
        1
    } else {
        content_width.div_ceil(width)
    }
}

fn visual_height(lines: &[Line<'_>], width: u16) -> usize {
    lines
        .iter()
        .map(|line| line_visual_height(line, width))
        .sum()
}

fn wrap_with_prefix(text: &str, width: usize, prefix: &str, style: Style) -> Vec<Line<'static>> {
    wrap_with_prefixes(text, width, prefix, prefix, style)
}

fn restyle_lines(template: &[Line<'static>], style: Style) -> Vec<Line<'static>> {
    template
        .iter()
        .map(|line| {
            if line.spans.is_empty() {
                return Line::default();
            }
            let spans: Vec<Span<'static>> = line
                .spans
                .iter()
                .map(|span| Span::styled(span.content.clone(), style))
                .collect();
            Line::from(spans)
        })
        .collect()
}

fn build_post_row_data(
    input: &PostRowInput,
    width: usize,
    score_width: usize,
    comments_width: usize,
) -> PostRowData {
    let identity_line = format!(
        "{ICON_SUBREDDIT} r/{}   {ICON_USER} u/{}",
        input.subreddit, input.author
    );
    let identity = wrap_plain(&identity_line, width, Style::default());

    let title = wrap_plain(&input.title, width, Style::default());

    let vote_marker = match input.vote {
        1 => "▲",
        -1 => "▼",
        _ => " ",
    };
    let metrics_line = format!(
        "{vote_marker} {ICON_UPVOTES} {:>score_width$}   {ICON_COMMENTS} {:>comments_width$}",
        input.score, input.comments
    );
    let metrics = wrap_plain(&metrics_line, width, Style::default());

    PostRowData {
        identity,
        title,
        metrics,
    }
}

fn sort_label(sort: reddit::SortOption) -> &'static str {
    match sort {
        reddit::SortOption::Hot => "/hot",
        reddit::SortOption::Best => "/best",
        reddit::SortOption::New => "/new",
        reddit::SortOption::Top => "/top",
        reddit::SortOption::Rising => "/rising",
    }
}

fn image_label(url: &str) -> String {
    Url::parse(url)
        .ok()
        .and_then(|parsed| {
            parsed
                .path_segments()
                .and_then(|mut segments| segments.next_back())
                .map(|segment| percent_decode_str(segment).decode_utf8_lossy().to_string())
        })
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "media".to_string())
}

fn env_truthy(key: &str) -> bool {
    env::var(key)
        .map(|value| matches!(value.trim(), "1" | "true" | "TRUE" | "True" | "yes" | "YES"))
        .unwrap_or(false)
}

fn running_inside_tmux() -> bool {
    let in_tmux = env::var("TMUX").map(|v| !v.is_empty()).unwrap_or(false)
        || env::var("TMUX_PANE")
            .map(|v| !v.is_empty())
            .unwrap_or(false);

    if in_tmux {
        return true;
    }

    env::var("TERM")
        .map(|term| term.to_ascii_lowercase().contains("tmux"))
        .unwrap_or(false)
}

fn is_kitty_terminal() -> bool {
    if env_truthy("REDDIX_DISABLE_KITTY") {
        return false;
    }
    if env_truthy("REDDIX_FORCE_KITTY") {
        return true;
    }
    let enable_override = env_truthy("REDDIX_ENABLE_KITTY");
    if running_inside_tmux() && !enable_override {
        return false;
    }
    if enable_override {
        return true;
    }
    if env::var("KITTY_WINDOW_ID")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    if env::var("WEZTERM_PANE")
        .map(|v| !v.is_empty())
        .unwrap_or(false)
    {
        return true;
    }
    if env::var("TERM_PROGRAM")
        .map(|term| term.to_lowercase().contains("wezterm"))
        .unwrap_or(false)
    {
        return true;
    }
    env::var("TERM")
        .map(|term| {
            let lower = term.to_lowercase();
            lower.contains("kitty") || lower.contains("wezterm")
        })
        .unwrap_or(false)
}

fn clamp_dimensions(width: i64, height: i64, max_width: i32, max_height: i32) -> (i32, i32) {
    let metrics = terminal_cell_metrics();
    let cell_width = metrics.width.max(1.0);
    let cell_height = metrics.height.max(1.0);

    let width_px = if width <= 0 {
        TARGET_PREVIEW_WIDTH_PX as f64
    } else {
        width as f64
    };
    let height_px = if height <= 0 {
        TARGET_PREVIEW_WIDTH_PX as f64
    } else {
        height as f64
    };

    let mut native_cols = width_px / cell_width;
    let mut native_rows = height_px / cell_height;
    if native_cols <= 0.0 {
        native_cols = max_width.max(1) as f64;
    }
    if native_rows <= 0.0 {
        native_rows = max_height.max(1) as f64;
    }

    let max_cols = max_width.max(1);
    let max_rows = max_height.max(1);
    let max_width_cells = max_cols as f64;
    let max_height_cells = max_rows as f64;
    let scale_x = max_width_cells / native_cols;
    let scale_y = max_height_cells / native_rows;
    let mut scale = scale_x.min(scale_y);
    if scale > 1.0 {
        scale = 1.0;
    }
    if scale <= 0.0 {
        scale = 1.0;
    }

    let cols = (native_cols * scale).round() as i32;
    let rows = (native_rows * scale).round() as i32;
    (cols.clamp(1, max_cols), rows.clamp(1, max_rows))
}

#[derive(Clone)]
pub struct Options {
    pub status_message: String,
    pub subreddits: Vec<String>,
    pub posts: Vec<PostPreview>,
    pub content: String,
    pub feed_service: Option<Arc<dyn FeedService + Send + Sync>>,
    pub subreddit_service: Option<Arc<dyn SubredditService + Send + Sync>>,
    pub default_sort: reddit::SortOption,
    pub comment_service: Option<Arc<dyn CommentService + Send + Sync>>,
    pub interaction_service: Option<Arc<dyn InteractionService + Send + Sync>>,
    pub media_handle: Option<media::Handle>,
    pub config_path: String,
    pub store: Arc<storage::Store>,
    pub session_manager: Option<Arc<session::Manager>>,
    pub fetch_subreddits_on_start: bool,
}

pub struct Model {
    status_message: String,
    subreddits: Vec<String>,
    posts: Vec<PostPreview>,
    feed_after: Option<String>,
    comments: Vec<CommentEntry>,
    visible_comment_indices: Vec<usize>,
    collapsed_comments: HashSet<usize>,
    content: Text<'static>,
    fallback_content: Text<'static>,
    fallback_source: String,
    content_source: String,
    media_previews: HashMap<String, MediaPreview>,
    media_failures: HashSet<String>,
    pending_media: HashMap<String, Arc<AtomicBool>>,
    media_layouts: HashMap<String, MediaLayout>,
    media_handle: Option<media::Handle>,
    feed_cache: HashMap<FeedCacheKey, FeedCacheEntry>,
    comment_cache: HashMap<String, CommentCacheEntry>,
    post_rows: HashMap<String, PostRowData>,
    post_rows_width: usize,
    pending_post_rows: Option<PendingPostRows>,
    content_cache: HashMap<String, Text<'static>>,
    pending_content: Option<PendingContent>,
    cache_scope: CacheScope,
    selected_sub: usize,
    selected_post: usize,
    selected_comment: usize,
    post_offset: Cell<usize>,
    post_view_height: Cell<u16>,
    comment_offset: Cell<usize>,
    comment_view_height: Cell<u16>,
    comment_view_width: Cell<u16>,
    comment_status_height: Cell<usize>,
    nav_index: usize,
    nav_mode: NavMode,
    content_scroll: u16,
    content_area: Option<Rect>,
    needs_kitty_flush: bool,
    pending_kitty_deletes: Vec<String>,
    active_kitty: Option<ActiveKitty>,
    interaction_service: Option<Arc<dyn InteractionService + Send + Sync>>,
    feed_service: Option<Arc<dyn FeedService + Send + Sync>>,
    subreddit_service: Option<Arc<dyn SubredditService + Send + Sync>>,
    comment_service: Option<Arc<dyn CommentService + Send + Sync>>,
    sort: reddit::SortOption,
    focused_pane: Pane,
    menu_visible: bool,
    menu_screen: MenuScreen,
    menu_form: MenuForm,
    menu_accounts: Vec<MenuAccountEntry>,
    menu_account_index: usize,
    link_menu_visible: bool,
    link_menu_items: Vec<LinkEntry>,
    link_menu_selected: usize,
    join_states: HashMap<i64, JoinState>,
    update_notice: Option<update::UpdateInfo>,
    update_check_in_progress: bool,
    update_checked: bool,
    current_version: Version,
    store: Arc<storage::Store>,
    session_manager: Option<Arc<session::Manager>>,
    login_in_progress: bool,
    needs_redraw: bool,
    numeric_jump: Option<NumericJump>,
    spinner: Spinner,
    config_path: String,
    comment_status: String,
    response_tx: Sender<AsyncResponse>,
    response_rx: Receiver<AsyncResponse>,
    next_request_id: u64,
    pending_posts: Option<PendingPosts>,
    pending_comments: Option<PendingComments>,
    pending_subreddits: Option<PendingSubreddits>,
}

impl Model {
    fn queue_update_check(&mut self) {
        if self.update_checked || self.update_check_in_progress {
            return;
        }
        if cfg!(test) || env::var(UPDATE_CHECK_DISABLE_ENV).is_ok() {
            self.update_checked = true;
            return;
        }
        self.update_checked = true;
        self.update_check_in_progress = true;
        let tx = self.response_tx.clone();
        let version = self.current_version.clone();
        thread::spawn(move || {
            let result = update::check_for_update(&version);
            let _ = tx.send(AsyncResponse::Update { result });
        });
    }

    fn active_account_id(&self) -> Option<i64> {
        self.session_manager
            .as_ref()
            .and_then(|manager| manager.active_account_id())
    }

    fn active_join_state(&self) -> Option<&JoinState> {
        let account_id = self.active_account_id()?;
        self.join_states.get(&account_id)
    }

    fn menu_account_positions(&self) -> MenuAccountPositions {
        let add = self.menu_accounts.len();
        let join = add + 1;
        let github = join + 1;
        let support = github + 1;
        let total = support + 1;
        MenuAccountPositions {
            add,
            join,
            github,
            support,
            total,
        }
    }

    fn queue_join_status_check(&mut self) {
        let Some(service) = self.interaction_service.clone() else {
            return;
        };
        let Some(account_id) = self.active_account_id() else {
            return;
        };
        self.join_states.entry(account_id).or_default();
        let tx = self.response_tx.clone();
        thread::spawn(move || {
            let result = service.is_subscribed(REDDIX_COMMUNITY);
            let _ = tx.send(AsyncResponse::JoinStatus { account_id, result });
        });
    }

    fn join_reddix_subreddit(&mut self) -> Result<()> {
        let Some(service) = self.interaction_service.clone() else {
            self.status_message = format!("Sign in to join {}.", REDDIX_COMMUNITY_DISPLAY);
            self.mark_dirty();
            return Ok(());
        };

        let Some(account_id) = self.active_account_id() else {
            self.status_message = "Select an account before joining the community.".to_string();
            self.mark_dirty();
            return Ok(());
        };

        let state = self.join_states.entry(account_id).or_default();
        if state.joined {
            self.status_message = format!("Already subscribed to {}.", REDDIX_COMMUNITY_DISPLAY);
            self.mark_dirty();
            return Ok(());
        }
        if state.pending {
            self.status_message = format!("Joining {} is already in progress...", REDDIX_COMMUNITY_DISPLAY);
            self.mark_dirty();
            return Ok(());
        }

        state.mark_pending();
        self.status_message = format!("Joining {}…", REDDIX_COMMUNITY_DISPLAY);
        self.mark_dirty();

        let tx = self.response_tx.clone();
        thread::spawn(move || {
            let result = service.subscribe(REDDIX_COMMUNITY);
            let _ = tx.send(AsyncResponse::JoinCommunity { account_id, result });
        });

        Ok(())
    }

    fn account_display_name(account: &storage::Account) -> String {
        if !account.display_name.trim().is_empty() {
            account.display_name.trim().to_string()
        } else if !account.username.trim().is_empty() {
            account.username.trim().to_string()
        } else {
            account.reddit_id.trim().to_string()
        }
    }

    fn refresh_menu_accounts(&mut self) -> Result<()> {
        let accounts = self
            .store
            .list_accounts()
            .context("list saved Reddit accounts")?;
        let active_id = self
            .session_manager
            .as_ref()
            .and_then(|manager| manager.active_account_id());
        self.menu_accounts = accounts
            .into_iter()
            .map(|account| MenuAccountEntry {
                id: account.id,
                display: Self::account_display_name(&account),
                is_active: active_id == Some(account.id),
            })
            .collect();
        let max_index = self.menu_accounts.len().saturating_add(2);
        self.menu_account_index = self.menu_account_index.min(max_index);
        Ok(())
    }

    fn current_cache_scope(&self) -> CacheScope {
        self.session_manager
            .as_ref()
            .and_then(|manager| manager.active_account_id())
            .map(CacheScope::Account)
            .unwrap_or(CacheScope::Anonymous)
    }

    fn ensure_cache_scope(&mut self) {
        let scope = self.current_cache_scope();
        self.adopt_cache_scope(scope);
    }

    fn adopt_cache_scope(&mut self, scope: CacheScope) {
        if self.cache_scope == scope {
            return;
        }
        self.cache_scope = scope;
        self.reset_scoped_caches();
    }

    fn reset_scoped_caches(&mut self) {
        if let Some(pending) = self.pending_posts.take() {
            pending.cancel_flag.store(true, Ordering::SeqCst);
        }
        if let Some(pending) = self.pending_comments.take() {
            pending.cancel_flag.store(true, Ordering::SeqCst);
        }
        if let Some(pending) = self.pending_content.take() {
            pending.cancel_flag.store(true, Ordering::SeqCst);
        }
        self.pending_post_rows = None;
        self.pending_subreddits = None;

        for flag in self.pending_media.values() {
            flag.store(true, Ordering::SeqCst);
        }
        self.pending_media.clear();

        self.feed_cache.clear();
        self.comment_cache.clear();
        self.content_cache.clear();
        self.post_rows.clear();
        self.post_rows_width = 0;

        self.media_previews.clear();
        self.media_layouts.clear();
        self.media_failures.clear();
        self.pending_kitty_deletes.clear();
        self.active_kitty = None;
        self.needs_kitty_flush = false;
        self.content_area = None;

        self.posts.clear();
        self.post_offset.set(0);
        self.numeric_jump = None;
        self.content_scroll = 0;
        self.content = self.fallback_content.clone();
        self.content_source = self.fallback_source.clone();

        self.comments.clear();
        self.collapsed_comments.clear();
        self.visible_comment_indices.clear();
        self.comment_offset.set(0);
        self.selected_comment = 0;
        self.comment_status = "Select a post to load comments.".to_string();

        self.selected_post = 0;

        self.reset_navigation_defaults();
        self.dismiss_link_menu(None);

        self.needs_redraw = true;
    }

    fn reset_navigation_defaults(&mut self) {
        self.subreddits = vec![
            "r/frontpage".to_string(),
            "r/all".to_string(),
            "r/popular".to_string(),
        ];
        self.selected_sub = 0;
        let nav_len = NAV_SORTS.len().saturating_add(self.subreddits.len());
        if nav_len > 0 {
            let desired = NAV_SORTS.len().saturating_add(self.selected_sub);
            self.nav_index = desired.min(nav_len - 1);
            self.nav_mode = NavMode::Subreddits;
        } else {
            self.nav_index = 0;
            self.nav_mode = NavMode::Sorts;
        }
    }

    fn scoped_comment_cache_mut(&mut self, key: &str) -> Option<&mut CommentCacheEntry> {
        if let Some(entry) = self.comment_cache.get(key) {
            if entry.scope != self.cache_scope {
                self.comment_cache.remove(key);
                return None;
            }
        }
        self.comment_cache.get_mut(key)
    }

    fn show_credentials_form(&mut self) -> Result<()> {
        self.menu_screen = MenuScreen::Credentials;
        self.menu_form = MenuForm::default();
        self.menu_form.focus(MenuField::ClientId);
        let mut error_message: Option<String> = None;
        match config::load(config::LoadOptions::default()) {
            Ok(cfg) => {
                let user_agent = if cfg.reddit.user_agent.trim().is_empty() {
                    config::RedditConfig::default().user_agent
                } else {
                    cfg.reddit.user_agent
                };
                self.menu_form.set_values(
                    cfg.reddit.client_id,
                    cfg.reddit.client_secret,
                    user_agent,
                );
            }
            Err(err) => {
                let default_agent = config::RedditConfig::default().user_agent;
                self.menu_form
                    .set_values(String::new(), String::new(), default_agent);
                let message = format!("Failed to load existing config: {err}");
                self.menu_form.set_status(message.clone());
                error_message = Some(message);
            }
        }
        self.status_message = match error_message {
            Some(msg) => format!("Edit Reddit credentials. {}", msg),
            None => "Edit Reddit credentials. Enter saves; Esc returns to accounts.".to_string(),
        };
        self.menu_account_index = self.menu_accounts.len();
        self.mark_dirty();
        Ok(())
    }

    fn switch_active_account(&mut self, account_id: i64) -> Result<()> {
        let manager = self.ensure_session_manager()?;
        let session = manager.switch(account_id)?;
        self.session_manager = Some(manager.clone());
        self.setup_authenticated_services()?;

        self.join_states.entry(session.account.id).or_default();
        self.queue_join_status_check();
        self.adopt_cache_scope(CacheScope::Account(session.account.id));

        self.refresh_menu_accounts().ok();

        let display = if !session.account.display_name.trim().is_empty() {
            session.account.display_name.trim().to_string()
        } else if !session.account.username.trim().is_empty() {
            session.account.username.trim().to_string()
        } else {
            session.account.reddit_id.trim().to_string()
        };

        self.status_message = format!("Switching to {}...", display);
        self.mark_dirty();

        if let Err(err) = self.reload_subreddits() {
            self.status_message = format!(
                "Switched to {}, but failed to refresh subreddits: {}",
                display, err
            );
            self.mark_dirty();
            return Err(err);
        }

        if let Err(err) = self.reload_posts() {
            self.status_message = format!(
                "Switched to {}, but failed to refresh posts: {}",
                display, err
            );
            self.mark_dirty();
            return Err(err);
        }

        self.status_message = format!("Switched to {}.", display);
        self.mark_dirty();
        Ok(())
    }
    fn mark_dirty(&mut self) {
        self.needs_redraw = true;
        if self.selected_post_has_kitty_preview() {
            self.needs_kitty_flush = true;
        }
    }

    fn focus_status_for(pane: Pane) -> String {
        match pane {
            Pane::Comments => {
                "Focused Comments pane — press c to fold threads (Shift+C expands all).".to_string()
            }
            _ => format!("Focused {} pane", pane.title()),
        }
    }

    fn active_kitty_matches(&self, post_name: &str) -> bool {
        self.active_kitty
            .as_ref()
            .is_some_and(|active| active.post_name == post_name)
    }

    fn prepare_active_kitty_delete(&mut self) -> Option<String> {
        let active = self.active_kitty.take()?;
        if let Some(preview) = self.media_previews.get_mut(&active.post_name) {
            if let Some(kitty) = preview.kitty_mut() {
                if kitty.id == active.image_id {
                    if !kitty.transmitted {
                        return None;
                    }
                    kitty.transmitted = false;
                    return Some(kitty.delete_sequence());
                }
            }
        }
        Some(KittyImage::delete_sequence_for(
            active.image_id,
            active.wrap_tmux,
        ))
    }

    fn queue_active_kitty_delete(&mut self) {
        if let Some(sequence) = self.prepare_active_kitty_delete() {
            self.pending_kitty_deletes.push(sequence);
            self.needs_kitty_flush = true;
            self.needs_redraw = true;
        }
    }

    fn emit_active_kitty_delete(
        &mut self,
        backend: &mut CrosstermBackend<Stdout>,
    ) -> io::Result<()> {
        if let Some(sequence) = self.prepare_active_kitty_delete() {
            crossterm::queue!(backend, Print(sequence))?;
            backend.flush()?;
        }
        Ok(())
    }

    fn flush_pending_kitty_deletes(
        &mut self,
        backend: &mut CrosstermBackend<Stdout>,
    ) -> io::Result<()> {
        if self.pending_kitty_deletes.is_empty() {
            return Ok(());
        }
        for sequence in self.pending_kitty_deletes.drain(..) {
            crossterm::queue!(backend, Print(sequence))?;
        }
        backend.flush()
    }

    fn selected_post_has_kitty_preview(&self) -> bool {
        self.posts
            .get(self.selected_post)
            .and_then(|post| self.media_previews.get(&post.post.name))
            .is_some_and(MediaPreview::has_kitty)
    }

    pub fn new(opts: Options) -> Self {
        let current_version =
            Version::parse(crate::VERSION).expect("crate version is valid semver");
        let markdown = markdown::Renderer::new();
        let fallback_content = markdown.render(&opts.content);
        let (response_tx, response_rx) = unbounded();
        let mut model = Self {
            status_message: opts.status_message.clone(),
            subreddits: opts.subreddits.clone(),
            posts: opts.posts.clone(),
            feed_after: None,
            comments: Vec::new(),
            visible_comment_indices: Vec::new(),
            collapsed_comments: HashSet::new(),
            content: fallback_content.clone(),
            fallback_content,
            fallback_source: opts.content.clone(),
            content_source: opts.content.clone(),
            media_previews: HashMap::new(),
            media_failures: HashSet::new(),
            pending_media: HashMap::new(),
            media_layouts: HashMap::new(),
            media_handle: opts.media_handle.clone(),
            feed_cache: HashMap::new(),
            comment_cache: HashMap::new(),
            post_rows: HashMap::new(),
            post_rows_width: 0,
            pending_post_rows: None,
            content_cache: HashMap::new(),
            pending_content: None,
            cache_scope: CacheScope::Anonymous,
            selected_sub: 0,
            selected_post: 0,
            selected_comment: 0,
            post_offset: Cell::new(0),
            post_view_height: Cell::new(0),
            comment_offset: Cell::new(0),
            comment_view_height: Cell::new(0),
            comment_view_width: Cell::new(0),
            comment_status_height: Cell::new(0),
            nav_index: 0,
            nav_mode: NavMode::Subreddits,
            content_scroll: 0,
            content_area: None,
            needs_kitty_flush: false,
            pending_kitty_deletes: Vec::new(),
            active_kitty: None,
            interaction_service: opts.interaction_service.clone(),
            feed_service: opts.feed_service.clone(),
            subreddit_service: opts.subreddit_service.clone(),
            comment_service: opts.comment_service.clone(),
            sort: opts.default_sort,
            focused_pane: Pane::Posts,
            menu_visible: false,
            menu_screen: MenuScreen::Accounts,
            menu_form: MenuForm::default(),
            menu_accounts: Vec::new(),
            menu_account_index: 0,
            link_menu_visible: false,
            link_menu_items: Vec::new(),
            link_menu_selected: 0,
            join_states: HashMap::new(),
            update_notice: None,
            update_check_in_progress: false,
            update_checked: false,
            current_version: current_version.clone(),
            store: opts.store.clone(),
            session_manager: opts.session_manager.clone(),
            login_in_progress: false,
            needs_redraw: true,
            numeric_jump: None,
            spinner: Spinner::new(),
            config_path: opts.config_path.clone(),
            comment_status: "Select a post to load comments.".to_string(),
            response_tx,
            response_rx,
            next_request_id: 1,
            pending_posts: None,
            pending_comments: None,
            pending_subreddits: None,
        };
        model.cache_scope = model.current_cache_scope();
        model.subreddits = model
            .subreddits
            .drain(..)
            .map(|name| normalize_subreddit_name(&name))
            .collect();
        if model.subreddits.is_empty() {
            model.subreddits = vec!["r/frontpage".into(), "r/all".into(), "r/popular".into()];
        } else {
            ensure_core_subreddits(&mut model.subreddits);
        }

        model.selected_sub = model
            .subreddits
            .iter()
            .position(|name| name.eq_ignore_ascii_case("r/frontpage"))
            .unwrap_or(0);
        model.selected_post = 0;
        model.selected_comment = 0;
        model.post_offset.set(0);
        model.comment_offset.set(0);
        model.comment_view_height.set(0);
        model.content_scroll = 0;

        if !model.posts.is_empty() {
            model.sync_content_from_selection();
        }

        let nav_len = NAV_SORTS.len().saturating_add(model.subreddits.len());
        if nav_len > 0 {
            let desired = NAV_SORTS.len().saturating_add(model.selected_sub);
            model.nav_index = desired.min(nav_len - 1);
            model.nav_mode = NavMode::Subreddits;
        } else {
            model.nav_index = 0;
            model.nav_mode = NavMode::Sorts;
        }

        if let Err(err) = model.reload_posts() {
            model.status_message = format!("Failed to load posts: {err}");
            model.content = model.fallback_content.clone();
            model.content_source = model.fallback_source.clone();
        }

        if opts.fetch_subreddits_on_start {
            if let Err(err) = model.reload_subreddits() {
                model.status_message = format!("Failed to refresh subreddits: {err}");
            }
        }

        model.ensure_post_visible();
        model.queue_update_check();
        model.queue_join_status_check();
        model
    }

    pub fn run(&mut self) -> Result<()> {
        let mut stdout = io::stdout();
        enable_raw_mode()?;
        stdout.execute(EnterAlternateScreen)?;
        let backend = CrosstermBackend::new(stdout);
        let mut terminal = Terminal::new(backend)?;
        terminal.clear()?;

        let result = self.event_loop(&mut terminal);

        disable_raw_mode()?;
        terminal.backend_mut().execute(LeaveAlternateScreen)?;
        terminal.show_cursor()?;

        result
    }

    fn event_loop(&mut self, terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
        let mut last_tick = Instant::now();
        let tick_rate = Duration::from_millis(120);

        loop {
            if self.poll_async() {
                self.mark_dirty();
            }

            if self.needs_redraw {
                terminal.draw(|frame| self.draw(frame))?;
                self.flush_inline_images(terminal.backend_mut())?;
                self.needs_redraw = false;
            }

            let timeout = tick_rate
                .checked_sub(last_tick.elapsed())
                .unwrap_or_else(|| Duration::from_millis(16));

            if event::poll(timeout)? {
                match event::read()? {
                    Event::Key(key) if key.kind == KeyEventKind::Press => {
                        match self.handle_key(key.code) {
                            Ok(true) => break,
                            Ok(false) => {}
                            Err(err) => {
                                self.status_message = format!("Error: {}", err);
                                self.mark_dirty();
                            }
                        }
                    }
                    Event::Mouse(mouse) => {
                        if let Err(err) = self.handle_mouse(mouse) {
                            self.status_message = format!("Error: {}", err);
                            self.mark_dirty();
                        }
                    }
                    _ => {}
                }
            }

            if self.poll_async() {
                self.mark_dirty();
            }

            if last_tick.elapsed() >= tick_rate {
                last_tick = Instant::now();
                let mut ticked = false;
                if self.is_loading() && self.spinner.advance() {
                    ticked = true;
                } else if !self.is_loading() {
                    self.spinner.reset();
                }
                if self.login_in_progress {
                    ticked = true;
                }
                if ticked {
                    self.mark_dirty();
                }
            }
        }

        Ok(())
    }

    fn visible_panes(&self) -> [Pane; 3] {
        match self.focused_pane {
            Pane::Navigation => [Pane::Navigation, Pane::Posts, Pane::Content],
            Pane::Posts | Pane::Content | Pane::Comments => {
                [Pane::Posts, Pane::Content, Pane::Comments]
            }
        }
    }

    fn commit_navigation_selection(&mut self) -> Result<()> {
        match self.nav_mode {
            NavMode::Sorts => {
                let target = self
                    .subreddits
                    .get(self.selected_sub)
                    .cloned()
                    .unwrap_or_else(|| "r/frontpage".to_string());
                self.status_message =
                    format!("Refreshing {} sorted by {}…", target, sort_label(self.sort));
                self.reload_posts()?;
            }
            NavMode::Subreddits => {
                if self.subreddits.is_empty() {
                    return Ok(());
                }
                let index = self.nav_index.min(self.subreddits.len().saturating_sub(1));
                if self.selected_sub != index {
                    self.selected_sub = index;
                    if let Some(name) = self.subreddits.get(index) {
                        self.status_message =
                            format!("Loading {} ({})…", name, sort_label(self.sort));
                    }
                    self.reload_posts()?;
                } else if let Some(name) = self.subreddits.get(index) {
                    self.status_message =
                        format!("{} is already loaded. Press r to refresh if needed.", name);
                }
            }
        }
        self.mark_dirty();
        Ok(())
    }

    fn handle_key(&mut self, code: KeyCode) -> Result<bool> {
        if self.menu_visible {
            return self.handle_menu_key(code);
        }

        if self.link_menu_visible {
            return self.handle_link_menu_key(code);
        }

        let mut dirty = false;

        if !matches!(code, KeyCode::Char(ch) if ch.is_ascii_digit()) {
            self.numeric_jump = None;
        }

        match code {
            KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
            KeyCode::Char('m') | KeyCode::Char('M') => {
                self.open_menu()?;
                dirty = true;
            }
            KeyCode::Char('r') | KeyCode::Char('R') => {
                self.reload_posts()?;
                dirty = true;
            }
            KeyCode::Char('s') => {
                self.reload_subreddits()?;
                dirty = true;
            }
            KeyCode::Char('o') | KeyCode::Char('O') => {
                self.open_link_menu();
            }
            KeyCode::Char('u') => {
                if self.focused_pane == Pane::Comments {
                    let new_dir = self
                        .selected_comment_index()
                        .and_then(|idx| self.comments.get(idx))
                        .map(|entry| toggle_vote_value(vote_from_likes(entry.likes), 1))
                        .unwrap_or(1);
                    self.vote_selected_comment(new_dir);
                } else {
                    let old = self
                        .posts
                        .get(self.selected_post)
                        .map(|post| vote_from_likes(post.post.likes))
                        .unwrap_or(0);
                    let new_dir = toggle_vote_value(old, 1);
                    self.vote_selected_post(new_dir);
                }
                dirty = true;
            }
            KeyCode::Char('d') => {
                if self.focused_pane == Pane::Comments {
                    let new_dir = self
                        .selected_comment_index()
                        .and_then(|idx| self.comments.get(idx))
                        .map(|entry| toggle_vote_value(vote_from_likes(entry.likes), -1))
                        .unwrap_or(-1);
                    self.vote_selected_comment(new_dir);
                } else {
                    let old = self
                        .posts
                        .get(self.selected_post)
                        .map(|post| vote_from_likes(post.post.likes))
                        .unwrap_or(0);
                    let new_dir = toggle_vote_value(old, -1);
                    self.vote_selected_post(new_dir);
                }
                dirty = true;
            }
            KeyCode::Char('c') => {
                if self.focused_pane == Pane::Comments {
                    self.toggle_selected_comment_fold();
                    dirty = true;
                }
            }
            KeyCode::Char('C') => {
                if self.focused_pane == Pane::Comments {
                    self.expand_all_comments();
                    dirty = true;
                }
            }
            KeyCode::Enter => {
                if self.focused_pane == Pane::Navigation {
                    self.commit_navigation_selection()?;
                    dirty = true;
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if self.focused_pane == Pane::Navigation && matches!(self.nav_mode, NavMode::Sorts)
                {
                    self.shift_sort(-1)?;
                    dirty = true;
                } else {
                    let previous = self.focused_pane.previous();
                    if previous != self.focused_pane {
                        self.focused_pane = previous;
                        if self.focused_pane == Pane::Navigation {
                            self.dismiss_link_menu(None);
                        }
                        self.status_message = Self::focus_status_for(self.focused_pane);
                        dirty = true;
                    }
                }
            }
            KeyCode::Char('l') | KeyCode::Right => {
                if self.focused_pane == Pane::Navigation && matches!(self.nav_mode, NavMode::Sorts)
                {
                    self.shift_sort(1)?;
                    dirty = true;
                } else {
                    let next = self.focused_pane.next();
                    if next != self.focused_pane {
                        self.focused_pane = next;
                        if self.focused_pane == Pane::Navigation {
                            self.dismiss_link_menu(None);
                        }
                        self.status_message = Self::focus_status_for(self.focused_pane);
                        dirty = true;
                    }
                }
            }
            KeyCode::Char(ch @ '1'..='5')
                if self.focused_pane == Pane::Navigation
                    && matches!(self.nav_mode, NavMode::Sorts) =>
            {
                let idx = (ch as u8 - b'1') as usize;
                self.set_sort_by_index(idx)?;
                dirty = true;
            }
            KeyCode::Char('j') | KeyCode::Down => {
                self.navigate_in_focus(1)?;
                dirty = true;
            }
            KeyCode::Char('k') | KeyCode::Up => {
                self.navigate_in_focus(-1)?;
                dirty = true;
            }
            KeyCode::PageDown | KeyCode::Char(' ') => {
                let step = if self.focused_pane == Pane::Posts {
                    self.posts_page_step()
                } else {
                    5
                };
                if step != 0 {
                    self.navigate_in_focus(step)?;
                    dirty = true;
                }
            }
            KeyCode::PageUp => {
                let step = if self.focused_pane == Pane::Posts {
                    self.posts_page_step()
                } else {
                    5
                };
                if step != 0 {
                    self.navigate_in_focus(-step)?;
                    dirty = true;
                }
            }
            KeyCode::Home => {
                if self.focused_pane == Pane::Posts {
                    if self.posts.is_empty() {
                        self.status_message = "No posts available to select.".to_string();
                    } else {
                        self.select_post_at(0);
                        dirty = true;
                        self.status_message = "Jumped to first post.".to_string();
                    }
                }
            }
            KeyCode::End => {
                if self.focused_pane == Pane::Posts {
                    if self.posts.is_empty() {
                        self.status_message = "No posts available to select.".to_string();
                    } else {
                        let last = self.posts.len() - 1;
                        self.select_post_at(last);
                        dirty = true;
                        self.status_message = format!("Jumped to post #{}.", last + 1);
                    }
                }
            }
            KeyCode::Char(ch) if ch.is_ascii_digit() => {
                if self.focused_pane == Pane::Posts {
                    let now = Instant::now();
                    let digit = ch.to_digit(10).unwrap() as usize;
                    let timeout = Duration::from_millis(800);
                    let (base, continuing) = match &self.numeric_jump {
                        Some(jump) if now.duration_since(jump.last_input) <= timeout => {
                            (jump.value, true)
                        }
                        _ => (0, false),
                    };
                    let new_value = if continuing {
                        base.saturating_mul(10).saturating_add(digit)
                    } else if digit == 0 {
                        10
                    } else {
                        digit
                    };
                    self.numeric_jump = Some(NumericJump {
                        value: new_value,
                        last_input: now,
                    });

                    if self.posts.is_empty() {
                        self.status_message = "No posts available to select.".to_string();
                    } else {
                        let max_index = self.posts.len() - 1;
                        let target = new_value.saturating_sub(1);
                        if target > max_index {
                            self.status_message = format!(
                                "Only {} post{} loaded right now.",
                                self.posts.len(),
                                if self.posts.len() == 1 {
                                    " is"
                                } else {
                                    "s are"
                                }
                            );
                        } else {
                            let previous = self.selected_post;
                            self.select_post_at(target);
                            self.status_message = if self.selected_post != previous {
                                format!("Selected post #{}.", self.selected_post + 1)
                            } else {
                                format!("Already on post #{}.", self.selected_post + 1)
                            };
                        }
                    }
                    dirty = true;
                }
            }
            _ => {}
        }

        if dirty {
            self.mark_dirty();
        }
        Ok(false)
    }

    fn handle_mouse(&mut self, event: MouseEvent) -> Result<()> {
        if self.menu_visible || self.link_menu_visible {
            return Ok(());
        }

        self.numeric_jump = None;

        match event.kind {
            MouseEventKind::ScrollDown => {
                self.navigate_in_focus(1)?;
                self.mark_dirty();
            }
            MouseEventKind::ScrollUp => {
                self.navigate_in_focus(-1)?;
                self.mark_dirty();
            }
            _ => {}
        }

        Ok(())
    }

    fn handle_menu_key(&mut self, code: KeyCode) -> Result<bool> {
        match self.menu_screen {
            MenuScreen::Accounts => self.handle_menu_accounts_key(code),
            MenuScreen::Credentials => self.handle_menu_credentials_key(code),
        }
    }

    fn handle_menu_accounts_key(&mut self, code: KeyCode) -> Result<bool> {
        let positions = self.menu_account_positions();
        let option_count = positions.total;
        let add_index = positions.add;
        let join_index = positions.join;
        let github_index = positions.github;
        let support_index = positions.support;

        if option_count == 0 {
            return Ok(false);
        }

        match code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('m') | KeyCode::Char('M') | KeyCode::Esc => {
                self.menu_visible = false;
                self.status_message = "Guided menu closed.".to_string();
                self.mark_dirty();
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.menu_account_index + 1 < option_count {
                    self.menu_account_index += 1;
                    self.mark_dirty();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.menu_account_index > 0 {
                    self.menu_account_index -= 1;
                    self.mark_dirty();
                }
            }
            KeyCode::Home => {
                self.menu_account_index = 0;
                self.mark_dirty();
            }
            KeyCode::End => {
                self.menu_account_index = option_count - 1;
                self.mark_dirty();
            }
            KeyCode::Char('a') | KeyCode::Char('A') => {
                self.menu_account_index = add_index;
                self.show_credentials_form()?;
            }
            KeyCode::Enter => {
                if self.menu_account_index < self.menu_accounts.len() {
                    let account_id = self.menu_accounts[self.menu_account_index].id;
                    match self.switch_active_account(account_id) {
                        Ok(()) => {
                            self.menu_visible = false;
                            self.mark_dirty();
                        }
                        Err(err) => {
                            self.status_message = format!("Failed to switch account: {err}");
                            self.mark_dirty();
                        }
                    }
                } else if self.menu_account_index == add_index {
                    self.show_credentials_form()?;
                } else if self.menu_account_index == join_index {
                    self.join_reddix_subreddit()?;
                } else if self.menu_account_index == github_index {
                    let _ = self.open_project_link();
                } else if self.menu_account_index == support_index {
                    let _ = self.open_support_link();
                }
            }
            _ => {}
        }
        Ok(false)
    }

    fn handle_menu_credentials_key(&mut self, code: KeyCode) -> Result<bool> {
        let mut dirty = false;
        match code {
            KeyCode::Char('q') => return Ok(true),
            KeyCode::Char('m') | KeyCode::Char('M') => {
                self.menu_visible = false;
                self.status_message = "Guided menu closed.".to_string();
                self.mark_dirty();
                return Ok(false);
            }
            KeyCode::Esc => {
                self.menu_screen = MenuScreen::Accounts;
                if let Err(err) = self.refresh_menu_accounts() {
                    self.status_message = format!("Guided menu: failed to list accounts: {}", err);
                } else {
                    if let Some(pos) = self.menu_accounts.iter().position(|entry| entry.is_active) {
                        self.menu_account_index = pos;
                    } else {
                        self.menu_account_index = 0;
                    }
                    self.status_message =
                        "Guided menu: j/k select account · Enter switch · a add · Esc/m close"
                            .to_string();
                }
                self.mark_dirty();
                return Ok(false);
            }
            KeyCode::Tab | KeyCode::Down => {
                self.menu_form.next();
                dirty = true;
            }
            KeyCode::BackTab | KeyCode::Up => {
                self.menu_form.previous();
                dirty = true;
            }
            KeyCode::Enter => match self.menu_form.active {
                MenuField::Save => {
                    let (client_id, client_secret, user_agent) = self.menu_form.trimmed_values();
                    match config::save_reddit_credentials(
                        None,
                        &client_id,
                        &client_secret,
                        &user_agent,
                    ) {
                        Ok(path) => {
                            self.menu_form.set_values(
                                client_id.clone(),
                                client_secret.clone(),
                                user_agent.clone(),
                            );
                            self.menu_form.focus(MenuField::ClientId);
                            if self.login_in_progress {
                                let message = "Authorization already in progress. Complete it in your browser.".to_string();
                                self.menu_form.set_status(message.clone());
                                self.status_message = message;
                            } else if let Err(err) = self.start_authorization_flow(path.as_path()) {
                                let message =
                                    format!("Failed to start Reddit authorization: {err}");
                                self.menu_form.set_status(message.clone());
                                self.status_message = message;
                            }
                            dirty = true;
                        }
                        Err(err) => {
                            let message = format!("Failed to save credentials: {err}");
                            self.menu_form.set_status(message.clone());
                            self.status_message = message;
                            dirty = true;
                        }
                    }
                }
                MenuField::CopyLink => {
                    if let Err(err) = self.copy_auth_link_to_clipboard() {
                        let message = format!("Failed to copy authorization link: {err}");
                        self.menu_form.set_status(message.clone());
                        self.status_message = message;
                    }
                    dirty = true;
                }
                _ => {
                    self.menu_form.next();
                    dirty = true;
                }
            },
            KeyCode::Backspace => {
                self.menu_form.backspace();
                dirty = true;
            }
            KeyCode::Delete => {
                self.menu_form.clear_active();
                dirty = true;
            }
            KeyCode::Char('c') | KeyCode::Char('C') => {
                if self.menu_form.has_auth_link() {
                    if let Err(err) = self.copy_auth_link_to_clipboard() {
                        let message = format!("Failed to copy authorization link: {err}");
                        self.menu_form.set_status(message.clone());
                        self.status_message = message;
                    }
                    dirty = true;
                }
            }
            KeyCode::Char(ch) => {
                if !ch.is_control() {
                    self.menu_form.insert_char(ch);
                    dirty = true;
                }
            }
            _ => {}
        }
        if dirty {
            self.mark_dirty();
        }
        Ok(false)
    }

    fn collect_links_for_current_context(&self) -> Vec<LinkEntry> {
        let mut seen = HashSet::new();
        let mut collected = Vec::new();

        if let Some(post) = self.posts.get(self.selected_post) {
            for entry in &post.links {
                if seen.insert(entry.url.clone()) {
                    collected.push(entry.clone());
                }
            }
        }

        if !self.visible_comment_indices.is_empty() {
            let selection = self
                .selected_comment
                .min(self.visible_comment_indices.len().saturating_sub(1));
            if let Some(comment_index) = self.visible_comment_indices.get(selection) {
                if let Some(comment) = self.comments.get(*comment_index) {
                    for entry in &comment.links {
                        if seen.insert(entry.url.clone()) {
                            collected.push(entry.clone());
                        }
                    }
                }
            }
        }

        collected
    }

    fn open_link_menu(&mut self) {
        let items = self.collect_links_for_current_context();
        if items.is_empty() {
            self.status_message = "No links available in the current context.".to_string();
            self.mark_dirty();
            return;
        }
        self.queue_active_kitty_delete();
        self.link_menu_items = items;
        self.link_menu_selected = 0;
        self.link_menu_visible = true;
        self.status_message = "Link menu: j/k move · Enter open · Esc closes".to_string();
        self.mark_dirty();
    }

    fn dismiss_link_menu(&mut self, message: Option<&str>) {
        if self.link_menu_visible {
            self.link_menu_visible = false;
            self.link_menu_items.clear();
            self.link_menu_selected = 0;
            if let Some(msg) = message {
                self.status_message = msg.to_string();
            }
            self.mark_dirty();
        }
    }

    fn handle_link_menu_key(&mut self, code: KeyCode) -> Result<bool> {
        if self.link_menu_items.is_empty() {
            if matches!(code, KeyCode::Esc | KeyCode::Char('q')) {
                self.dismiss_link_menu(Some("Link menu closed."));
            }
            return Ok(false);
        }

        match code {
            KeyCode::Esc | KeyCode::Char('q') => {
                self.dismiss_link_menu(Some("Link menu closed."));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.link_menu_selected > 0 {
                    self.link_menu_selected -= 1;
                    self.mark_dirty();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.link_menu_selected + 1 < self.link_menu_items.len() {
                    self.link_menu_selected += 1;
                    self.mark_dirty();
                }
            }
            KeyCode::PageUp => {
                if self.link_menu_selected > 0 {
                    let step = self.link_menu_selected.min(5);
                    self.link_menu_selected -= step;
                    self.mark_dirty();
                }
            }
            KeyCode::PageDown => {
                if self.link_menu_selected + 1 < self.link_menu_items.len() {
                    let remaining = self.link_menu_items.len() - self.link_menu_selected - 1;
                    let step = remaining.min(5);
                    self.link_menu_selected += step;
                    self.mark_dirty();
                }
            }
            KeyCode::Enter | KeyCode::Char('o') | KeyCode::Char('O') => {
                if let Err(err) = self.open_selected_link() {
                    self.status_message = format!("Failed to open link: {err}");
                    self.mark_dirty();
                }
            }
            _ => {}
        }

        Ok(false)
    }

    fn open_selected_link(&mut self) -> Result<()> {
        if self.link_menu_items.is_empty() {
            self.status_message = "No link selected.".to_string();
            return Ok(());
        }

        let index = self
            .link_menu_selected
            .min(self.link_menu_items.len().saturating_sub(1));
        let entry = &self.link_menu_items[index];
        let label = entry.label.clone();
        let url = entry.url.clone();

        match webbrowser::open(&url) {
            Ok(_) => {
                let message = format!("Opened {label} in your browser.");
                self.dismiss_link_menu(None);
                self.status_message = message;
                self.mark_dirty();
            }
            Err(err) => {
                self.status_message = format!("Failed to open {label}: {err} (URL: {url})");
                self.mark_dirty();
            }
        }

        Ok(())
    }

    fn open_support_link(&mut self) -> Result<()> {
        match webbrowser::open(SUPPORT_LINK_URL) {
            Ok(_) => {
                self.status_message = "Opened support page in your browser.".to_string();
                self.mark_dirty();
                Ok(())
            }
            Err(err) => {
                let message = format!("Failed to open support page: {err}");
                self.status_message = message.clone();
                self.mark_dirty();
                Err(anyhow!(message))
            }
        }
    }

    fn open_project_link(&mut self) -> Result<()> {
        match webbrowser::open(PROJECT_LINK_URL) {
            Ok(_) => {
                self.status_message = "Opened project page on GitHub.".to_string();
                self.mark_dirty();
                Ok(())
            }
            Err(err) => {
                let message = format!("Failed to open project page: {err}");
                self.status_message = message.clone();
                self.mark_dirty();
                Err(anyhow!(message))
            }
        }
    }
    fn open_menu(&mut self) -> Result<()> {
        self.menu_form = MenuForm::default();
        self.menu_screen = MenuScreen::Accounts;
        self.queue_active_kitty_delete();
        self.dismiss_link_menu(None);
        self.menu_visible = true;

        let status = match self.refresh_menu_accounts() {
            Ok(_) => {
                if let Some(pos) = self.menu_accounts.iter().position(|entry| entry.is_active) {
                    self.menu_account_index = pos;
                } else {
                    self.menu_account_index = 0;
                }
                if self.menu_accounts.is_empty() {
                    "Guided menu: no Reddit accounts found. Press a to add one.".to_string()
                } else {
                    "Guided menu: j/k select account · Enter switch · a add · Esc/m close"
                        .to_string()
                }
            }
            Err(err) => {
                self.menu_accounts.clear();
                self.menu_account_index = 0;
                format!("Guided menu: failed to list accounts: {}", err)
            }
        };

        self.status_message = status;
        self.mark_dirty();
        Ok(())
    }

    fn ensure_session_manager(&mut self) -> Result<Arc<session::Manager>> {
        if let Some(manager) = &self.session_manager {
            return Ok(manager.clone());
        }

        let mut cfg = config::load(config::LoadOptions::default()).context("load config")?;
        if cfg.reddit.client_id.trim().is_empty() {
            bail!("Reddit client ID is required before starting authorization");
        }
        if cfg.reddit.user_agent.trim().is_empty() {
            cfg.reddit.user_agent = config::RedditConfig::default().user_agent;
        }
        if cfg.reddit.scopes.is_empty() {
            cfg.reddit.scopes = config::RedditConfig::default().scopes;
        }

        let flow_cfg = auth::Config {
            client_id: cfg.reddit.client_id.clone(),
            client_secret: cfg.reddit.client_secret.clone(),
            scope: cfg.reddit.scopes.clone(),
            user_agent: cfg.reddit.user_agent.clone(),
            auth_url: "https://www.reddit.com/api/v1/authorize".into(),
            token_url: "https://www.reddit.com/api/v1/access_token".into(),
            identity_url: "https://oauth.reddit.com/api/v1/me".into(),
            redirect_uri: cfg.reddit.redirect_uri.clone(),
            refresh_skew: Duration::from_secs(30),
        };

        let flow =
            Arc::new(auth::Flow::new(self.store.clone(), flow_cfg).context("create auth flow")?);
        let manager = Arc::new(
            session::Manager::new(self.store.clone(), flow).context("create session manager")?,
        );
        self.session_manager = Some(manager.clone());
        Ok(manager)
    }

    fn start_authorization_flow(&mut self, saved_path: &Path) -> Result<()> {
        let manager = self.ensure_session_manager()?;
        let authz = manager
            .begin_login()
            .context("start Reddit authorization")?;
        let url = authz.browser_url.clone();

        self.login_in_progress = true;
        self.menu_form.authorization_started(url.clone());

        let mut message = format!("Saved Reddit credentials to {}. ", saved_path.display());

        match webbrowser::open(&url) {
            Ok(_) => {
                message.push_str(
                    "Authorize Reddix in your browser, then return here once it finishes. If nothing opened automatically, copy the link shown below.",
                );
            }
            Err(err) => {
                message.push_str(&format!(
                    "Open {} in your browser to authorize (auto-open failed: {}).",
                    url, err
                ));
            }
        }

        self.menu_form.set_status(message.clone());
        self.status_message = message;

        let tx = self.response_tx.clone();
        let manager_clone = manager.clone();
        thread::spawn(move || {
            let result = manager_clone
                .complete_login(authz)
                .map(|session| session.account.username);
            let _ = tx.send(AsyncResponse::Login { result });
        });

        if let Err(err) = self.copy_auth_link_to_clipboard() {
            let warning = format!("Browser launched, but copying the link failed: {}", err);
            self.menu_form.set_status(warning.clone());
            self.status_message = warning;
        }

        self.mark_dirty();
        Ok(())
    }

    fn copy_auth_link_to_clipboard(&mut self) -> Result<()> {
        let Some(url) = self.menu_form.auth_link().map(|s| s.to_string()) else {
            bail!("authorization link unavailable");
        };
        let mut clipboard =
            ClipboardContext::new().map_err(|err| anyhow!("create clipboard context: {}", err))?;
        clipboard
            .set_contents(url.clone())
            .map_err(|err| anyhow!("copy authorization link: {}", err))?;
        let message = "Authorization link copied to clipboard.".to_string();
        self.menu_form.set_status(message.clone());
        self.status_message = message;
        self.mark_dirty();
        Ok(())
    }

    fn setup_authenticated_services(&mut self) -> Result<()> {
        let manager = self.ensure_session_manager()?;
        let cfg = config::load(config::LoadOptions::default()).context("load config")?;
        let user_agent = if cfg.reddit.user_agent.trim().is_empty() {
            config::RedditConfig::default().user_agent
        } else {
            cfg.reddit.user_agent.clone()
        };
        let token_provider = manager
            .active_token_provider()
            .context("retrieve active Reddit session")?;
        let client = Arc::new(
            reddit::Client::new(
                token_provider,
                reddit::ClientConfig {
                    user_agent,
                    base_url: None,
                    http_client: None,
                },
            )
            .context("create reddit client")?,
        );

        self.feed_service = Some(Arc::new(crate::data::RedditFeedService::new(
            client.clone(),
        )));
        self.subreddit_service = Some(Arc::new(crate::data::RedditSubredditService::new(
            client.clone(),
        )));
        self.comment_service = Some(Arc::new(crate::data::RedditCommentService::new(
            client.clone(),
        )));
        self.interaction_service =
            Some(Arc::new(crate::data::RedditInteractionService::new(client)));
        Ok(())
    }

    fn handle_login_success(&mut self, username: String) -> Result<()> {
        self.menu_form.authorization_complete();
        let message = format!(
            "Authorization complete. Signed in as {}. Loading Reddit data...",
            username
        );
        self.menu_form.set_status(message.clone());
        self.status_message = message;

        self.setup_authenticated_services()?;
        self.ensure_cache_scope();

        if let Err(err) = self.reload_subreddits() {
            let msg = format!("Failed to refresh subreddits: {err}");
            self.menu_form.set_status(msg.clone());
            self.status_message = msg;
        }
        if let Err(err) = self.reload_posts() {
            let msg = format!("Failed to reload posts: {err}");
            self.menu_form.set_status(msg.clone());
            self.status_message = msg;
        }

        if let Err(err) = self.refresh_menu_accounts() {
            self.status_message = format!(
                "Signed in as {}, but failed to refresh account list: {}",
                username, err
            );
        } else {
            self.menu_screen = MenuScreen::Accounts;
            if let Some(pos) = self.menu_accounts.iter().position(|entry| entry.is_active) {
                self.menu_account_index = pos;
            } else if !self.menu_accounts.is_empty() {
                self.menu_account_index = 0;
            }
        }

        if let Some(account_id) = self.active_account_id() {
            self.join_states.entry(account_id).or_default();
            self.queue_join_status_check();
        }

        self.mark_dirty();
        Ok(())
    }

    fn poll_async(&mut self) -> bool {
        let mut changed = false;
        while let Ok(message) = self.response_rx.try_recv() {
            self.handle_async_response(message);
            changed = true;
        }
        changed
    }

    fn handle_async_response(&mut self, message: AsyncResponse) {
        match message {
            AsyncResponse::Posts {
                request_id,
                target,
                sort,
                result,
            } => {
                let Some(pending) = &self.pending_posts else {
                    return;
                };
                if pending.cancel_flag.load(Ordering::SeqCst) {
                    return;
                }
                if pending.request_id != request_id {
                    return;
                }
                let mode = pending.mode;
                self.pending_posts = None;
                if matches!(mode, LoadMode::Replace) {
                    self.pending_comments = None;
                }

                match result {
                    Ok(batch) => {
                        let key = FeedCacheKey::new(&target, sort);
                        self.apply_posts_batch(&target, sort, batch, false, mode);
                        if !self.posts.is_empty() {
                            let snapshot = PostBatch {
                                posts: self.posts.clone(),
                                after: self.feed_after.clone(),
                            };
                            self.cache_posts(key, snapshot);
                        }
                    }
                    Err(err) => {
                        self.status_message = format!("Failed to load posts: {err}");
                    }
                }
                self.mark_dirty();
            }
            AsyncResponse::PostRows {
                request_id,
                width,
                rows,
            } => {
                let Some(pending) = &self.pending_post_rows else {
                    return;
                };
                if pending.request_id != request_id || pending.width != width {
                    return;
                }
                self.pending_post_rows = None;
                if self.post_rows_width != width {
                    self.post_rows.clear();
                    self.post_rows_width = width;
                }
                for (name, data) in rows {
                    self.post_rows.insert(name, data);
                }
                if let Some(selected) = self.posts.get(self.selected_post) {
                    let key = selected.post.name.clone();
                    if self.content_cache.contains_key(&key) && self.post_rows.contains_key(&key) {
                        self.sync_content_from_selection();
                    }
                }
                self.mark_dirty();
            }
            AsyncResponse::Comments {
                request_id,
                post_name,
                result,
            } => {
                let Some(pending) = &self.pending_comments else {
                    return;
                };
                if pending.cancel_flag.load(Ordering::SeqCst)
                    || pending.request_id != request_id
                    || pending.post_name != post_name
                {
                    return;
                }
                let current_name = self
                    .posts
                    .get(self.selected_post)
                    .map(|post| post.post.name.as_str());
                if current_name != Some(post_name.as_str()) {
                    return;
                }
                self.pending_comments = None;

                match result {
                    Ok(comments) => {
                        self.cache_comments(&post_name, comments.clone());
                        self.comments = comments;
                        self.collapsed_comments.clear();
                        self.selected_comment = 0;
                        self.comment_offset.set(0);
                        self.rebuild_visible_comments_reset();
                        self.recompute_comment_status();
                    }
                    Err(err) => {
                        self.comments.clear();
                        self.collapsed_comments.clear();
                        self.visible_comment_indices.clear();
                        self.selected_comment = 0;
                        self.comment_offset.set(0);
                        self.comment_status = format!("Failed to load comments: {err}");
                    }
                }
                self.dismiss_link_menu(None);
                self.mark_dirty();
            }
            AsyncResponse::Content {
                request_id,
                post_name,
                rendered,
            } => {
                let Some(pending) = &self.pending_content else {
                    return;
                };
                if pending.request_id != request_id || pending.post_name != post_name {
                    return;
                }
                if pending.cancel_flag.load(Ordering::SeqCst) {
                    return;
                }
                self.pending_content = None;
                let cache_entry = rendered.clone();
                self.content_cache.insert(post_name.clone(), cache_entry);
                let target_post = self
                    .posts
                    .iter()
                    .find(|candidate| candidate.post.name == post_name)
                    .cloned();
                if let Some(ref post) = target_post {
                    self.content = self.compose_content(rendered.clone(), post);
                    self.ensure_media_request_ready(post);
                } else {
                    self.content = rendered;
                }
                self.mark_dirty();
            }
            AsyncResponse::Subreddits { request_id, result } => {
                let Some(pending) = &self.pending_subreddits else {
                    return;
                };
                if pending.request_id != request_id {
                    return;
                }
                self.pending_subreddits = None;

                match result {
                    Ok(names) => {
                        let previous = self
                            .subreddits
                            .get(self.selected_sub)
                            .cloned()
                            .unwrap_or_else(|| "r/frontpage".to_string());

                        self.subreddits = names
                            .into_iter()
                            .map(|name| normalize_subreddit_name(&name))
                            .collect();
                        ensure_core_subreddits(&mut self.subreddits);

                        if let Some(idx) = self
                            .subreddits
                            .iter()
                            .position(|candidate| candidate.eq_ignore_ascii_case(previous.as_str()))
                        {
                            self.selected_sub = idx;
                        } else if self.selected_sub >= self.subreddits.len() {
                            self.selected_sub = 0;
                        }
                        self.nav_index = self
                            .selected_sub
                            .min(self.subreddits.len().saturating_sub(1));
                        self.nav_mode = NavMode::Subreddits;
                        self.status_message = "Subreddits refreshed".to_string();
                        if let Err(err) = self.reload_posts() {
                            self.status_message = format!("Failed to reload posts: {err}");
                        }
                    }
                    Err(err) => {
                        self.status_message = format!("Failed to refresh subreddits: {err}");
                    }
                }
                self.mark_dirty();
            }
            AsyncResponse::Media { post_name, result } => {
                self.pending_media.remove(&post_name);
                let relevant = self.posts.iter().any(|post| post.post.name == post_name);
                if !relevant {
                    return;
                }
                match result {
                    Ok(Some(preview)) => {
                        self.media_failures.remove(&post_name);
                        self.media_previews.insert(post_name.clone(), preview);
                    }
                    Ok(None) => {
                        self.media_failures.insert(post_name.clone());
                        self.media_previews.remove(&post_name);
                    }
                    Err(err) => {
                        self.media_failures.insert(post_name.clone());
                        self.media_previews.remove(&post_name);
                        self.status_message = format!("Image preview failed: {}", err);
                    }
                }

                let current = self
                    .posts
                    .get(self.selected_post)
                    .map(|post| post.post.name.as_str());
                if current == Some(post_name.as_str()) {
                    self.sync_content_from_selection();
                }
                self.mark_dirty();
            }
            AsyncResponse::Login { result } => {
                self.login_in_progress = false;
                match result {
                    Ok(username) => {
                        if let Err(err) = self.handle_login_success(username) {
                            let message = format!(
                                "Authorization completed but initializing Reddit client failed: {}",
                                err
                            );
                            self.menu_form.authorization_complete();
                            self.menu_form.set_status(message.clone());
                            self.status_message = message;
                        }
                    }
                    Err(err) => {
                        self.menu_form.authorization_complete();
                        let message = format!("Authorization failed: {}", err);
                        self.menu_form.set_status(message.clone());
                        self.status_message = message;
                    }
                }
                self.mark_dirty();
            }
            AsyncResponse::Update { result } => {
                self.update_check_in_progress = false;
                self.update_checked = true;
                match result {
                    Ok(Some(info)) => {
                        self.update_notice = Some(info);
                    }
                    Ok(None) => {
                        self.update_notice = None;
                    }
                    Err(err) => {
                        self.update_notice = None;
                        self.status_message = format!("Update check failed: {}", err);
                    }
                }
                self.mark_dirty();
            }
            AsyncResponse::JoinStatus { account_id, result } => {
                let state = self.join_states.entry(account_id).or_default();
                match result {
                    Ok(joined) => {
                        state.pending = false;
                        state.joined = joined;
                        state.last_error = None;
                    }
                    Err(err) => {
                        state.mark_error(err.to_string());
                        self.status_message = format!("Checking {} subscription failed: {}", REDDIX_COMMUNITY_DISPLAY, err);
                    }
                }
                self.mark_dirty();
            }
            AsyncResponse::JoinCommunity { account_id, result } => {
                let state = self.join_states.entry(account_id).or_default();
                state.pending = false;
                match result {
                    Ok(()) => {
                        state.mark_success();
                        self.status_message = format!("Joined {}. Thanks for supporting the community!", REDDIX_COMMUNITY_DISPLAY);
                    }
                    Err(err) => {
                        let message = format!("Joining {} failed: {}", REDDIX_COMMUNITY_DISPLAY, err);
                        state.mark_error(message.clone());
                        self.status_message = message;
                    }
                }
                self.mark_dirty();
            }
            AsyncResponse::VoteResult {
                target,
                requested,
                previous,
                error,
            } => {
                let action_word = match requested {
                    1 => ("Upvoted", "upvote"),
                    -1 => ("Downvoted", "downvote"),
                    _ => ("Cleared vote on", "clear vote on"),
                };
                match target {
                    VoteTarget::Post { fullname } => {
                        if let Some(post) = self
                            .posts
                            .iter_mut()
                            .find(|candidate| candidate.post.name == fullname)
                        {
                            let current_vote = vote_from_likes(post.post.likes);
                            if let Some(err) = error {
                                if current_vote == requested {
                                    if requested != previous {
                                        post.post.score += (previous - requested) as i64;
                                    }
                                    post.post.likes = likes_from_vote(previous);
                                    self.post_rows.remove(&fullname);
                                }
                                self.status_message = format!(
                                    "Failed to {} \"{}\": {}",
                                    action_word.1, post.post.title, err
                                );
                            } else {
                                self.status_message =
                                    format!("{} \"{}\".", action_word.0, post.post.title);
                                self.post_rows.remove(&fullname);
                            }
                            self.mark_dirty();
                        }
                    }
                    VoteTarget::Comment { fullname } => {
                        let mut cache_update = None;
                        if let Some((index, comment)) = self
                            .comments
                            .iter_mut()
                            .enumerate()
                            .find(|(_, entry)| entry.name == fullname)
                        {
                            let current_vote = vote_from_likes(comment.likes);
                            if let Some(err) = error {
                                if current_vote == requested {
                                    if requested != previous {
                                        comment.score += (previous - requested) as i64;
                                    }
                                    comment.likes = likes_from_vote(previous);
                                }
                                self.status_message = format!(
                                    "Failed to {} comment by u/{}: {}",
                                    action_word.1, comment.author, err
                                );
                            } else {
                                self.status_message =
                                    format!("{} comment by u/{}.", action_word.0, comment.author);
                            }
                            cache_update = Some((index, comment.score, comment.likes));
                            self.ensure_comment_visible();
                            self.mark_dirty();
                        }
                        if let Some((index, score, likes)) = cache_update {
                            if let Some(post_name) = self
                                .posts
                                .get(self.selected_post)
                                .map(|post| post.post.name.clone())
                            {
                                if let Some(cache) = self.scoped_comment_cache_mut(&post_name) {
                                    if let Some(entry) = cache.comments.get_mut(index) {
                                        entry.score = score;
                                        entry.likes = likes;
                                    }
                                    cache.fetched_at = Instant::now();
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    fn navigate_in_focus(&mut self, delta: i32) -> Result<()> {
        match self.focused_pane {
            Pane::Navigation => match self.nav_mode {
                NavMode::Sorts => {
                    if delta > 0 && !self.subreddits.is_empty() {
                        self.nav_mode = NavMode::Subreddits;
                        self.nav_index = self
                            .selected_sub
                            .min(self.subreddits.len().saturating_sub(1));
                        self.status_message =
                            "Use j/k inside the list, Enter to load; press k on the first subreddit to return to sort.".to_string();
                    }
                }
                NavMode::Subreddits => {
                    if self.subreddits.is_empty() {
                        return Ok(());
                    }

                    let len = self.subreddits.len() as i32;
                    let current = self.nav_index as i32;
                    let next = (current + delta).clamp(0, len.saturating_sub(1));

                    if delta < 0 && current == 0 && next == 0 {
                        self.nav_mode = NavMode::Sorts;
                        self.status_message = format!(
                            "Sort row selected ({sort}). Use ←/→ or 1-5 to change, Enter reloads.",
                            sort = sort_label(self.sort)
                        );
                    } else if next != current {
                        self.nav_index = next as usize;
                        if let Some(name) = self.subreddits.get(self.nav_index) {
                            self.status_message = format!(
                                "Highlighted {} · {} — press Enter to load.",
                                name,
                                sort_label(self.sort)
                            );
                        }
                    }
                }
            },
            Pane::Posts => {
                if self.posts.is_empty() {
                    return Ok(());
                }
                let len = self.posts.len() as i32;
                let current = self.selected_post as i32;
                let next = (current + delta).clamp(0, len.saturating_sub(1));
                self.select_post_at(next as usize);
            }
            Pane::Content => {
                if delta > 0 {
                    self.content_scroll = self.content_scroll.saturating_add(delta as u16);
                } else {
                    let magnitude = (-delta) as u16;
                    self.content_scroll = self.content_scroll.saturating_sub(magnitude);
                }
                if self.selected_post_has_kitty_preview() {
                    self.needs_kitty_flush = true;
                }
            }
            Pane::Comments => {
                if self.visible_comment_indices.is_empty() {
                    return Ok(());
                }
                let len = self.visible_comment_indices.len() as i32;
                let current = self.selected_comment as i32;
                let next = (current + delta).clamp(0, len.saturating_sub(1));
                if next != current {
                    self.selected_comment = next as usize;
                    self.ensure_comment_visible();
                }
            }
        }
        Ok(())
    }

    fn vote_selected_post(&mut self, dir: i32) {
        let service = match self.interaction_service.as_ref() {
            Some(service) => Arc::clone(service),
            None => {
                self.status_message =
                    "Voting requires a signed-in Reddit session (press m to log in).".to_string();
                return;
            }
        };

        if self.posts.is_empty() {
            self.status_message = "No posts available to vote on.".to_string();
        } else {
            let index = self.selected_post.min(self.posts.len().saturating_sub(1));
            let fullname = self.posts[index].post.name.clone();
            if fullname.is_empty() {
                self.status_message = "Unable to vote on this post.".to_string();
                return;
            }
            let title = self.posts[index].post.title.clone();
            let action_word = match dir {
                1 => "Upvoted",
                -1 => "Downvoted",
                _ => "Cleared vote on",
            };
            let new_vote = dir.clamp(-1, 1);
            let old_vote = if let Some(post) = self.posts.get(index) {
                vote_from_likes(post.post.likes)
            } else {
                0
            };
            if let Some(post) = self.posts.get_mut(index) {
                if old_vote != new_vote {
                    post.post.score += (new_vote - old_vote) as i64;
                }
                post.post.likes = likes_from_vote(new_vote);
            }
            self.post_rows.remove(&fullname);
            self.status_message = format!("{} \"{}\" (sending...)", action_word, title);
            self.mark_dirty();

            let tx = self.response_tx.clone();
            let requested = new_vote;
            let previous = old_vote;
            thread::spawn(move || {
                let error = service
                    .vote(fullname.as_str(), dir)
                    .err()
                    .map(|err| err.to_string());
                let _ = tx.send(AsyncResponse::VoteResult {
                    target: VoteTarget::Post {
                        fullname: fullname.clone(),
                    },
                    requested,
                    previous,
                    error,
                });
            });
        }
    }

    fn vote_selected_comment(&mut self, dir: i32) {
        let service = match self.interaction_service.as_ref() {
            Some(service) => Arc::clone(service),
            None => {
                self.status_message =
                    "Voting requires a signed-in Reddit session (press m to log in).".to_string();
                return;
            }
        };

        if self.visible_comment_indices.is_empty() {
            self.status_message = "No comments available to vote on.".to_string();
            return;
        }

        let visible_len = self.visible_comment_indices.len();
        let selection = self.selected_comment.min(visible_len.saturating_sub(1));
        let comment_index = match self.visible_comment_indices.get(selection) {
            Some(index) => *index,
            None => {
                self.status_message = "Comment selection is out of sync.".to_string();
                return;
            }
        };

        let (fullname, author) = match self.comments.get(comment_index) {
            Some(comment) => (comment.name.clone(), comment.author.clone()),
            None => {
                self.status_message = "Comment selection is out of sync.".to_string();
                return;
            }
        };

        if fullname.is_empty() {
            self.status_message = "Unable to vote on this comment.".to_string();
            return;
        }
        let action_word = match dir {
            1 => "Upvoted",
            -1 => "Downvoted",
            _ => "Cleared vote on",
        };

        let new_vote = dir.clamp(-1, 1);
        let old_vote = if let Some(entry) = self.comments.get(comment_index) {
            vote_from_likes(entry.likes)
        } else {
            0
        };

        let mut updated_comment = None;
        if let Some(entry) = self.comments.get_mut(comment_index) {
            if old_vote != new_vote {
                entry.score += (new_vote - old_vote) as i64;
            }
            entry.likes = likes_from_vote(new_vote);
            updated_comment = Some((entry.score, entry.likes));
        }

        if let Some((score, likes)) = updated_comment {
            if let Some(post_name) = self
                .posts
                .get(self.selected_post)
                .map(|post| post.post.name.clone())
            {
                if let Some(cache) = self.scoped_comment_cache_mut(&post_name) {
                    if let Some(entry) = cache.comments.get_mut(comment_index) {
                        entry.score = score;
                        entry.likes = likes;
                    }
                    cache.fetched_at = Instant::now();
                }
            }
        }

        self.ensure_comment_visible();
        self.status_message = format!("{} comment by u/{} (sending...)", action_word, author);
        self.mark_dirty();

        let tx = self.response_tx.clone();
        thread::spawn(move || {
            let error = service
                .vote(fullname.as_str(), dir)
                .err()
                .map(|err| err.to_string());
            let _ = tx.send(AsyncResponse::VoteResult {
                target: VoteTarget::Comment {
                    fullname: fullname.clone(),
                },
                requested: new_vote,
                previous: old_vote,
                error,
            });
        });
    }

    fn selected_comment_index(&self) -> Option<usize> {
        self.visible_comment_indices
            .get(self.selected_comment)
            .copied()
    }

    fn rebuild_visible_comments_internal(
        &mut self,
        preferred: Option<usize>,
        fallback_to_first: bool,
    ) {
        self.visible_comment_indices.clear();
        let mut new_selection = None;
        let mut hidden_depths: Vec<usize> = Vec::new();

        for (index, comment) in self.comments.iter().enumerate() {
            while hidden_depths
                .last()
                .is_some_and(|depth| *depth >= comment.depth)
            {
                hidden_depths.pop();
            }

            if !hidden_depths.is_empty() {
                continue;
            }

            let visible_index = self.visible_comment_indices.len();
            self.visible_comment_indices.push(index);

            if self.collapsed_comments.contains(&index) {
                hidden_depths.push(comment.depth);
            }

            if preferred == Some(index) {
                new_selection = Some(visible_index);
            }
        }

        if let Some(selection) = new_selection {
            self.selected_comment = selection;
        } else if self.visible_comment_indices.is_empty() || fallback_to_first {
            self.selected_comment = 0;
        } else {
            self.selected_comment = self
                .selected_comment
                .min(self.visible_comment_indices.len() - 1);
        }

        self.ensure_comment_visible();
    }

    fn rebuild_visible_comments_reset(&mut self) {
        self.rebuild_visible_comments_internal(None, true);
    }

    fn recompute_comment_status(&mut self) {
        if self.comments.is_empty() {
            self.comment_status = "No comments yet.".to_string();
            return;
        }

        let total = self.comments.len();
        let visible = self.visible_comment_indices.len();
        if visible == total {
            self.comment_status = format!("{total} comments loaded");
        } else {
            let hidden = total.saturating_sub(visible);
            self.comment_status =
                format!("{total} comments loaded · {visible} visible · {hidden} hidden",);
        }
    }

    fn toggle_selected_comment_fold(&mut self) {
        let Some(comment_index) = self.selected_comment_index() else {
            self.status_message = "No comment selected to fold.".to_string();
            return;
        };

        let entry = match self.comments.get(comment_index) {
            Some(entry) => entry,
            None => {
                self.status_message = "Comment selection is out of sync.".to_string();
                return;
            }
        };

        if entry.descendant_count == 0 {
            self.status_message = "Comment has no replies to fold.".to_string();
            return;
        }

        if self.collapsed_comments.remove(&comment_index) {
            self.status_message = "Expanded comment thread.".to_string();
        } else {
            self.collapsed_comments.insert(comment_index);
            let replies = entry.descendant_count;
            let suffix = if replies == 1 { "reply" } else { "replies" };
            self.status_message = format!("Collapsed {replies} {suffix}.");
        }

        self.rebuild_visible_comments_internal(Some(comment_index), false);
        self.recompute_comment_status();
        self.mark_dirty();
    }

    fn expand_all_comments(&mut self) {
        if self.collapsed_comments.is_empty() {
            self.status_message = "All comments already expanded.".to_string();
            return;
        }

        let preferred = self.selected_comment_index();
        self.collapsed_comments.clear();
        self.rebuild_visible_comments_internal(preferred, false);
        self.recompute_comment_status();
        self.status_message = "Expanded all comment threads.".to_string();
        self.mark_dirty();
    }

    fn select_post_at(&mut self, index: usize) {
        if self.posts.is_empty() {
            self.selected_post = 0;
            self.post_offset.set(0);
            return;
        }

        let max_index = self.posts.len() - 1;
        let clamped = index.min(max_index);
        let changed = clamped != self.selected_post;

        self.selected_post = clamped;
        if changed {
            self.queue_active_kitty_delete();
            self.comment_offset.set(0);
            self.dismiss_link_menu(None);
            self.sync_content_from_selection();
            if let Err(err) = self.load_comments_for_selection() {
                self.comment_status = format!("Failed to load comments: {err}");
            }
        }

        self.ensure_post_visible();
        self.maybe_request_more_posts();
    }

    fn maybe_request_more_posts(&mut self) {
        if self.posts.is_empty() {
            return;
        }
        if self.pending_posts.is_some() {
            return;
        }
        let Some(after) = self.feed_after.as_ref() else {
            return;
        };
        if after.trim().is_empty() {
            return;
        }
        let remaining = self
            .posts
            .len()
            .saturating_sub(self.selected_post.saturating_add(1));
        if remaining > POST_PRELOAD_THRESHOLD {
            return;
        }
        if let Err(err) = self.load_more_posts() {
            self.status_message = format!("Failed to load more posts: {err}");
        }
    }

    fn current_feed_target(&self) -> String {
        if self.subreddits.is_empty() {
            return "r/frontpage".to_string();
        }
        let index = self
            .selected_sub
            .min(self.subreddits.len().saturating_sub(1));
        self.subreddits
            .get(index)
            .cloned()
            .unwrap_or_else(|| "r/frontpage".to_string())
    }

    fn select_subreddit_by_name(&mut self, name: &str) -> bool {
        if self.subreddits.is_empty() {
            return false;
        }
        if let Some(idx) = self
            .subreddits
            .iter()
            .position(|candidate| candidate.eq_ignore_ascii_case(name))
        {
            self.selected_sub = idx;
            let nav_base = NAV_SORTS.len();
            let nav_len = nav_base.saturating_add(self.subreddits.len());
            let desired = nav_base.saturating_add(idx);
            if nav_len > 0 {
                self.nav_index = desired.min(nav_len.saturating_sub(1));
            } else {
                self.nav_index = 0;
            }
            self.nav_mode = NavMode::Subreddits;
            true
        } else {
            false
        }
    }

    fn ensure_post_visible(&self) {
        let len = self.posts.len();
        if len == 0 {
            self.post_offset.set(0);
            return;
        }

        if self.post_view_height.get() == 0 {
            self.post_offset.set(self.selected_post.min(len - 1));
            return;
        }

        let selected = self.selected_post.min(len - 1);
        let mut height_cache: Vec<Option<usize>> = vec![None; selected + 1];
        let mut height_for = |idx: usize| -> usize {
            if idx > selected {
                return 0;
            }
            if let Some(height) = height_cache[idx] {
                return height;
            }
            let height = self.post_item_height(idx).max(1);
            height_cache[idx] = Some(height);
            height
        };

        let mut prefix = vec![0usize; selected.saturating_add(1) + 1];
        for idx in 0..=selected {
            prefix[idx + 1] = prefix[idx].saturating_add(height_for(idx));
        }

        let span_height = |start: usize, end: usize| -> usize {
            if start > end {
                return 0;
            }
            prefix[end + 1].saturating_sub(prefix[start])
        };

        let mut best_choice: Option<(usize, f32)> = None;
        let mut fallback_choice: Option<(usize, f32)> = None;

        for candidate in 0..=selected {
            let available = self.available_post_height(candidate);
            if available == 0 {
                continue;
            }

            let selection_height = height_for(selected);
            let bottom = span_height(candidate, selected);
            if bottom > available {
                continue;
            }

            let top = bottom.saturating_sub(selection_height);
            let center = top as f32 + (selection_height as f32 / 2.0);
            let lower_bound = (available as f32) * 0.25;
            let upper_bound = (available as f32) * 0.75;
            let midpoint = (available as f32) * 0.5;
            let diff = (center - midpoint).abs();

            if center >= lower_bound && center <= upper_bound {
                match best_choice {
                    Some((_, best_diff)) if diff >= best_diff => {}
                    _ => best_choice = Some((candidate, diff)),
                }
            }

            match fallback_choice {
                Some((_, best_diff)) if diff >= best_diff => {}
                _ => fallback_choice = Some((candidate, diff)),
            }
        }

        let chosen = best_choice
            .or(fallback_choice)
            .map(|(candidate, _)| candidate)
            .unwrap_or(selected);

        self.post_offset.set(chosen);
    }

    fn ensure_comment_visible(&self) {
        let len = self.visible_comment_indices.len();
        if len == 0 {
            self.comment_offset.set(0);
            return;
        }

        if self.comment_view_height.get() <= 1 {
            self.comment_offset.set(0);
            return;
        }

        let available = self.available_comment_height();
        if available == 0 {
            self.comment_offset.set(0);
            return;
        }

        let selected = self.selected_comment.min(len - 1);
        let mut height_cache: Vec<Option<usize>> = vec![None; selected.saturating_add(1)];
        let mut height_for = |idx: usize| -> usize {
            if idx > selected {
                return 0;
            }
            if let Some(height) = height_cache.get(idx).and_then(|cached| *cached) {
                return height;
            }
            let height = self.comment_item_height(idx).max(1);
            if let Some(slot) = height_cache.get_mut(idx) {
                *slot = Some(height);
            }
            height
        };

        let mut prefix = vec![0usize; selected.saturating_add(1) + 1];
        for idx in 0..=selected {
            prefix[idx + 1] = prefix[idx].saturating_add(height_for(idx));
        }

        let span_height = |start: usize, end: usize| -> usize {
            if start > end {
                return 0;
            }
            prefix[end + 1].saturating_sub(prefix[start])
        };

        let mut best_choice: Option<(usize, f32)> = None;
        let mut fallback_choice: Option<(usize, f32)> = None;
        let selection_height = height_for(selected);
        let lower_bound = (available as f32) * 0.25;
        let upper_bound = (available as f32) * 0.75;
        let midpoint = (available as f32) * 0.5;

        for candidate in 0..=selected {
            let bottom = span_height(candidate, selected);
            if bottom > available {
                continue;
            }
            let top = bottom.saturating_sub(selection_height);
            let center = top as f32 + (selection_height as f32 / 2.0);
            let diff = (center - midpoint).abs();

            if center >= lower_bound && center <= upper_bound {
                match best_choice {
                    Some((_, best_diff)) if diff >= best_diff => {}
                    _ => best_choice = Some((candidate, diff)),
                }
            }

            match fallback_choice {
                Some((_, best_diff)) if diff >= best_diff => {}
                _ => fallback_choice = Some((candidate, diff)),
            }
        }

        let chosen = best_choice
            .or(fallback_choice)
            .map(|(candidate, _)| candidate)
            .unwrap_or(selected);

        self.comment_offset.set(chosen);
    }

    fn post_item_height(&self, index: usize) -> usize {
        let Some(post) = self.posts.get(index) else {
            return 0;
        };
        if let Some(row) = self.post_rows.get(&post.post.name) {
            row.identity
                .len()
                .saturating_add(row.title.len())
                .saturating_add(row.metrics.len())
                .saturating_add(1)
        } else {
            3
        }
    }

    fn available_post_height(&self, offset: usize) -> usize {
        let mut base = self.post_view_height.get() as usize;
        if base == 0 {
            return 0;
        }
        if offset == 0 && self.update_notice.is_some() {
            base = base.saturating_sub(UPDATE_BANNER_HEIGHT);
        }
        if offset == 0 && self.pending_posts.is_some() && !self.posts.is_empty() {
            base = base.saturating_sub(POST_LOADING_HEADER_HEIGHT);
        }
        base
    }

    fn comment_item_height(&self, visible_index: usize) -> usize {
        let Some(&comment_index) = self.visible_comment_indices.get(visible_index) else {
            return 0;
        };
        let Some(comment) = self.comments.get(comment_index) else {
            return 0;
        };
        let width = self.comment_view_width.get().max(1) as usize;
        let collapsed = self.collapsed_comments.contains(&comment_index);
        let indicator = if collapsed { "[+]" } else { "[-]" };
        let meta_style = Style::default();
        let body_style = Style::default();
        let lines = comment_lines(comment, width, indicator, meta_style, body_style, collapsed);
        lines.len().saturating_add(1)
    }

    fn available_comment_height(&self) -> usize {
        let total = self.comment_view_height.get() as usize;
        if total == 0 {
            return 0;
        }
        let status_height = self.comment_status_height.get().min(total);
        total.saturating_sub(status_height)
    }

    fn posts_page_step(&self) -> i32 {
        let visible = self.post_view_height.get();
        let visible = if visible == 0 { 1 } else { visible as usize };
        let step = visible.saturating_sub(1).max(1);
        step as i32
    }

    fn set_sort_by_index(&mut self, index: usize) -> Result<()> {
        if index >= NAV_SORTS.len() {
            return Ok(());
        }
        let sort = NAV_SORTS[index];
        if self.sort != sort {
            self.sort = sort;
            self.reload_posts()?;
        }
        self.nav_mode = NavMode::Sorts;
        self.status_message = format!("Sort set to {}", sort_label(sort));
        Ok(())
    }

    fn shift_sort(&mut self, delta: i32) -> Result<()> {
        let len = NAV_SORTS.len() as i32;
        if len == 0 {
            return Ok(());
        }
        let current = NAV_SORTS
            .iter()
            .position(|candidate| *candidate == self.sort)
            .unwrap_or(0) as i32;
        let next = (current + delta).rem_euclid(len);
        self.set_sort_by_index(next as usize)
    }

    fn cache_posts(&mut self, key: FeedCacheKey, batch: PostBatch) {
        if self.feed_cache.len() >= FEED_CACHE_MAX {
            if let Some(old_key) = self
                .feed_cache
                .iter()
                .min_by_key(|(_, entry)| entry.fetched_at)
                .map(|(key, _)| key.clone())
            {
                self.feed_cache.remove(&old_key);
            }
        }
        self.feed_cache.insert(
            key,
            FeedCacheEntry {
                batch,
                fetched_at: Instant::now(),
                scope: self.cache_scope,
            },
        );
    }

    fn cache_comments(&mut self, post_name: &str, comments: Vec<CommentEntry>) {
        if self.comment_cache.len() >= COMMENT_CACHE_MAX {
            if let Some(old_key) = self
                .comment_cache
                .iter()
                .min_by_key(|(_, entry)| entry.fetched_at)
                .map(|(key, _)| key.clone())
            {
                self.comment_cache.remove(&old_key);
            }
        }
        self.comment_cache.insert(
            post_name.to_string(),
            CommentCacheEntry {
                comments,
                fetched_at: Instant::now(),
                scope: self.cache_scope,
            },
        );
    }

    fn apply_posts_batch(
        &mut self,
        target: &str,
        sort: reddit::SortOption,
        mut batch: PostBatch,
        from_cache: bool,
        mode: LoadMode,
    ) {
        match mode {
            LoadMode::Replace => {
                if batch.posts.is_empty() {
                    if !from_cache {
                        if let Some(fallback) = fallback_feed_target(target) {
                            if self.select_subreddit_by_name(fallback) {
                                self.feed_after = None;
                                self.status_message = format!(
                                    "No posts available for {} ({}) — loading {} instead...",
                                    target,
                                    sort_label(sort),
                                    fallback
                                );
                                if let Err(err) = self.reload_posts() {
                                    self.status_message = format!(
                                        "Tried {}, but failed to load posts: {}",
                                        fallback, err
                                    );
                                }
                                self.mark_dirty();
                                return;
                            }
                        }
                    }

                    let source = if from_cache { "(cached)" } else { "" };
                    self.status_message = format!(
                        "No posts available for {} ({}) {}",
                        target,
                        sort_label(sort),
                        source
                    )
                    .trim()
                    .to_string();
                    self.queue_active_kitty_delete();
                    self.posts.clear();
                    self.feed_after = batch.after.take();
                    self.post_offset.set(0);
                    self.numeric_jump = None;
                    self.content_scroll = 0;
                    self.content = self.fallback_content.clone();
                    self.content_source = self.fallback_source.clone();
                    self.comments.clear();
                    self.collapsed_comments.clear();
                    self.visible_comment_indices.clear();
                    self.comment_offset.set(0);
                    self.comment_status = "No comments available.".to_string();
                    self.selected_comment = 0;
                    self.dismiss_link_menu(None);
                    self.content_cache.clear();
                    self.post_rows.clear();
                    self.post_rows_width = 0;
                    self.pending_post_rows = None;
                    self.media_previews.clear();
                    self.media_layouts.clear();
                    self.media_failures.clear();
                    self.needs_kitty_flush = false;
                    self.content_area = None;
                    if let Some(pending) = self.pending_content.take() {
                        pending.cancel_flag.store(true, Ordering::SeqCst);
                    }
                    return;
                }

                self.status_message = if from_cache {
                    format!(
                        "Loaded {} posts from {} ({}) — cached",
                        batch.posts.len(),
                        target,
                        sort_label(sort)
                    )
                } else {
                    format!(
                        "Loaded {} posts from {} ({})",
                        batch.posts.len(),
                        target,
                        sort_label(sort)
                    )
                };
                self.queue_active_kitty_delete();
                self.posts = batch.posts;
                self.feed_after = batch.after;
                self.post_offset.set(0);
                self.comment_offset.set(0);
                self.dismiss_link_menu(None);
                self.numeric_jump = None;
                self.media_previews
                    .retain(|key, _| self.posts.iter().any(|post| post.post.name == *key));
                self.media_layouts
                    .retain(|key, _| self.posts.iter().any(|post| post.post.name == *key));
                self.media_failures
                    .retain(|key| self.posts.iter().any(|post| post.post.name == *key));
                self.pending_media.retain(|key, flag| {
                    let keep = self.posts.iter().any(|post| post.post.name == *key);
                    if !keep {
                        flag.store(true, Ordering::SeqCst);
                    }
                    keep
                });
                self.comment_cache
                    .retain(|key, _| self.posts.iter().any(|post| post.post.name == *key));
                self.content_cache
                    .retain(|key, _| self.posts.iter().any(|post| post.post.name == *key));
                self.post_rows
                    .retain(|key, _| self.posts.iter().any(|post| post.post.name == *key));
                self.pending_post_rows = None;
                self.post_rows_width = 0;
                if let Some(pending) = self.pending_content.take() {
                    pending.cancel_flag.store(true, Ordering::SeqCst);
                }
                self.selected_post = 0;
                self.sync_content_from_selection();
                self.selected_comment = 0;
                self.comment_status = "Loading comments...".to_string();
                self.comments.clear();
                self.collapsed_comments.clear();
                self.visible_comment_indices.clear();
                self.comment_offset.set(0);
                if let Err(err) = self.load_comments_for_selection() {
                    self.comment_status = format!("Failed to load comments: {err}");
                }
                self.ensure_post_visible();
            }
            LoadMode::Append => {
                let previous_after = self.feed_after.clone();
                self.feed_after = batch.after.clone();
                if batch.posts.is_empty() {
                    if self.feed_after.is_none() || self.feed_after == previous_after {
                        if self.feed_after.is_none() {
                            self.status_message =
                                format!("Reached end of {} ({})", target, sort_label(sort));
                        } else {
                            self.status_message = format!(
                                "No additional posts returned for {} ({}).",
                                target,
                                sort_label(sort)
                            );
                        }
                    } else {
                        self.status_message = format!(
                            "No additional posts returned yet for {} ({}); requesting more...",
                            target,
                            sort_label(sort)
                        );
                        self.maybe_request_more_posts();
                    }
                    return;
                }

                let mut seen: HashSet<String> = self
                    .posts
                    .iter()
                    .map(|post| post.post.name.clone())
                    .collect();
                let original_incoming = batch.posts.len();
                batch
                    .posts
                    .retain(|post| seen.insert(post.post.name.clone()));

                if batch.posts.is_empty() {
                    if self.feed_after.is_none() || self.feed_after == previous_after {
                        if self.feed_after.is_none() {
                            self.status_message =
                                format!("Reached end of {} ({})", target, sort_label(sort));
                        } else {
                            self.status_message = format!(
                                "Skipped {} duplicate post{} from {} ({}).",
                                original_incoming,
                                if original_incoming == 1 { "" } else { "s" },
                                target,
                                sort_label(sort)
                            );
                        }
                    } else {
                        self.status_message = format!(
                            "Skipped {} duplicate post{} from {} ({}); requesting more...",
                            original_incoming,
                            if original_incoming == 1 { "" } else { "s" },
                            target,
                            sort_label(sort)
                        );
                        self.maybe_request_more_posts();
                    }
                    return;
                }

                let added = batch.posts.len();
                self.posts.extend(batch.posts);
                self.status_message = format!(
                    "Loaded {} more posts from {} ({}) — {} total.",
                    added,
                    target,
                    sort_label(sort),
                    self.posts.len()
                );
                self.ensure_post_visible();
                self.maybe_request_more_posts();
            }
        }
    }

    fn is_loading(&self) -> bool {
        self.pending_posts.is_some()
            || self.pending_comments.is_some()
            || self.pending_content.is_some()
            || self.login_in_progress
            || !self.pending_media.is_empty()
    }

    fn reload_posts(&mut self) -> Result<()> {
        self.ensure_cache_scope();
        let Some(service) = &self.feed_service else {
            self.pending_posts = None;
            self.pending_comments = None;
            self.queue_active_kitty_delete();
            self.posts.clear();
            self.feed_after = None;
            self.post_offset.set(0);
            self.numeric_jump = None;
            self.content_scroll = 0;
            self.content = self.fallback_content.clone();
            self.content_source = self.fallback_source.clone();
            self.status_message = "Sign in to load Reddit posts.".to_string();
            self.comments.clear();
            self.collapsed_comments.clear();
            self.visible_comment_indices.clear();
            self.comment_offset.set(0);
            self.comment_status = "Sign in to load comments.".to_string();
            self.media_previews.clear();
            self.media_layouts.clear();
            self.media_failures.clear();
            for flag in self.pending_media.values() {
                flag.store(true, Ordering::SeqCst);
            }
            self.pending_media.clear();
            self.needs_kitty_flush = false;
            self.content_area = None;
            return Ok(());
        };

        let target = self.current_feed_target();
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let sort = self.sort;
        let cache_key = FeedCacheKey::new(&target, sort);

        if let Some(entry) = self.feed_cache.get(&cache_key) {
            if entry.scope == self.cache_scope && entry.fetched_at.elapsed() < FEED_CACHE_TTL {
                if let Some(pending) = self.pending_posts.take() {
                    pending.cancel_flag.store(true, Ordering::SeqCst);
                }
                self.pending_posts = None;
                if let Some(pending) = self.pending_comments.take() {
                    pending.cancel_flag.store(true, Ordering::SeqCst);
                }
                self.apply_posts_batch(&target, sort, entry.batch.clone(), true, LoadMode::Replace);
                self.mark_dirty();
                return Ok(());
            }
        }

        if let Some(pending) = self.pending_posts.take() {
            pending.cancel_flag.store(true, Ordering::SeqCst);
        }
        if let Some(pending) = self.pending_comments.take() {
            pending.cancel_flag.store(true, Ordering::SeqCst);
        }

        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.pending_posts = Some(PendingPosts {
            request_id,
            cancel_flag: cancel_flag.clone(),
            mode: LoadMode::Replace,
        });
        self.feed_after = None;
        self.status_message = format!("Loading {} ({})...", target, sort_label(sort));
        self.spinner.reset();

        let tx = self.response_tx.clone();
        let service = service.clone();
        let subreddit = if is_front_page(&target) {
            None
        } else {
            Some(
                target
                    .trim_start_matches("r/")
                    .trim_start_matches('/')
                    .to_string(),
            )
        };
        let target_for_thread = target.clone();
        let opts = reddit::ListingOptions {
            after: None,
            ..Default::default()
        };

        thread::spawn(move || {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let result = match subreddit.as_ref() {
                Some(name) => service
                    .load_subreddit(name, sort, opts.clone())
                    .map(|listing| PostBatch {
                        after: listing.after,
                        posts: listing
                            .children
                            .into_iter()
                            .map(|thing| make_preview(thing.data))
                            .collect::<Vec<_>>(),
                    }),
                None => service
                    .load_front_page(sort, opts.clone())
                    .map(|listing| PostBatch {
                        after: listing.after,
                        posts: listing
                            .children
                            .into_iter()
                            .map(|thing| make_preview(thing.data))
                            .collect::<Vec<_>>(),
                    }),
            };

            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }

            let _ = tx.send(AsyncResponse::Posts {
                request_id,
                target: target_for_thread,
                sort,
                result,
            });
        });
        Ok(())
    }

    fn load_more_posts(&mut self) -> Result<()> {
        self.ensure_cache_scope();
        if self.pending_posts.is_some() {
            return Ok(());
        }
        let Some(after) = self.feed_after.clone() else {
            return Ok(());
        };
        if after.trim().is_empty() {
            return Ok(());
        }
        let Some(service) = &self.feed_service else {
            return Ok(());
        };

        let target = self.current_feed_target();
        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let sort = self.sort;

        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.pending_posts = Some(PendingPosts {
            request_id,
            cancel_flag: cancel_flag.clone(),
            mode: LoadMode::Append,
        });
        self.status_message = format!(
            "Loading more posts from {} ({})...",
            target,
            sort_label(sort)
        );
        self.spinner.reset();

        let tx = self.response_tx.clone();
        let service = service.clone();
        let subreddit = if is_front_page(&target) {
            None
        } else {
            Some(
                target
                    .trim_start_matches("r/")
                    .trim_start_matches('/')
                    .to_string(),
            )
        };
        let target_for_thread = target.clone();
        let opts = reddit::ListingOptions {
            after: Some(after),
            ..Default::default()
        };

        thread::spawn(move || {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let result = match subreddit.as_ref() {
                Some(name) => service
                    .load_subreddit(name, sort, opts.clone())
                    .map(|listing| PostBatch {
                        after: listing.after,
                        posts: listing
                            .children
                            .into_iter()
                            .map(|thing| make_preview(thing.data))
                            .collect::<Vec<_>>(),
                    }),
                None => service
                    .load_front_page(sort, opts.clone())
                    .map(|listing| PostBatch {
                        after: listing.after,
                        posts: listing
                            .children
                            .into_iter()
                            .map(|thing| make_preview(thing.data))
                            .collect::<Vec<_>>(),
                    }),
            };

            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }

            let _ = tx.send(AsyncResponse::Posts {
                request_id,
                target: target_for_thread,
                sort,
                result,
            });
        });

        Ok(())
    }

    fn reload_subreddits(&mut self) -> Result<()> {
        self.ensure_cache_scope();
        let Some(service) = &self.subreddit_service else {
            self.pending_subreddits = None;
            self.status_message = "Subreddit list unavailable without login.".to_string();
            return Ok(());
        };

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.pending_subreddits = Some(PendingSubreddits { request_id });
        self.status_message = "Refreshing subreddit list...".to_string();

        let tx = self.response_tx.clone();
        let service = service.clone();
        thread::spawn(move || {
            let result = service
                .list_subreddits(reddit::SubredditSource::Subscriptions)
                .map(|listing| listing.into_iter().map(|sub| sub.name).collect::<Vec<_>>());
            let _ = tx.send(AsyncResponse::Subreddits { request_id, result });
        });

        Ok(())
    }

    fn sync_content_from_selection(&mut self) {
        self.content_scroll = 0;
        self.needs_kitty_flush = false;
        if let Some(post) = self.posts.get(self.selected_post).cloned() {
            let key = post.post.name.clone();
            let source = content_from_post(&post);
            self.content_source = source.clone();

            if self
                .pending_content
                .as_ref()
                .map(|pending| pending.post_name != key)
                .unwrap_or(false)
            {
                if let Some(pending) = self.pending_content.take() {
                    pending.cancel_flag.store(true, Ordering::SeqCst);
                }
            }

            if let Some(cached) = self.content_cache.get(&key).cloned() {
                self.content = self.compose_content(cached, &post);
                self.ensure_media_request_ready(&post);
            } else {
                let placeholder = Text::from(vec![Line::from(Span::styled(
                    "Rendering content...",
                    Style::default().fg(COLOR_TEXT_SECONDARY),
                ))]);
                self.content = self.compose_content(placeholder, &post);
                self.queue_content_render(key, source);
            }
        } else {
            self.content_source = self.fallback_source.clone();
            self.content = self.fallback_content.clone();
        }
    }

    fn request_media_preview(&mut self, post: &reddit::Post) {
        let key = post.name.clone();
        if self.pending_media.contains_key(&key)
            || self.media_previews.contains_key(&key)
            || self.media_failures.contains(&key)
        {
            return;
        }

        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.pending_media.insert(key.clone(), cancel_flag.clone());
        let tx = self.response_tx.clone();
        let post_clone = post.clone();
        let media_handle = self.media_handle.clone();

        thread::spawn(move || {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let name = post_clone.name.clone();
            let result = load_media_preview(&post_clone, cancel_flag.as_ref(), media_handle);
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let _ = tx.send(AsyncResponse::Media {
                post_name: name,
                result,
            });
        });
    }

    fn ensure_media_request_ready(&mut self, post: &PostPreview) {
        let key = post.post.name.clone();
        if self.media_failures.contains(&key)
            || self.media_previews.contains_key(&key)
            || self.pending_media.contains_key(&key)
        {
            return;
        }
        if !self.post_rows.contains_key(&key) {
            return;
        }
        if !self.content_cache.contains_key(&key) {
            return;
        }
        self.request_media_preview(&post.post);
    }

    fn load_comments_for_selection(&mut self) -> Result<()> {
        self.ensure_cache_scope();
        let Some(service) = self.comment_service.clone() else {
            self.comments.clear();
            self.collapsed_comments.clear();
            self.visible_comment_indices.clear();
            self.comment_offset.set(0);
            self.comment_status = "Sign in to load comments.".to_string();
            self.pending_comments = None;
            self.dismiss_link_menu(None);
            return Ok(());
        };

        let Some(post) = self.posts.get(self.selected_post) else {
            self.comments.clear();
            self.collapsed_comments.clear();
            self.visible_comment_indices.clear();
            self.comment_offset.set(0);
            self.comment_status = "Select a post to load comments.".to_string();
            self.pending_comments = None;
            self.dismiss_link_menu(None);
            return Ok(());
        };

        let key = post.post.name.clone();
        let subreddit = post.post.subreddit.clone();
        let article = post.post.id.clone();
        if let Some(entry) = self.comment_cache.get(&key) {
            if entry.scope == self.cache_scope && entry.fetched_at.elapsed() < COMMENT_CACHE_TTL {
                self.comments = entry.comments.clone();
                self.collapsed_comments.clear();
                self.selected_comment = 0;
                self.comment_offset.set(0);
                self.rebuild_visible_comments_reset();
                if self.comments.is_empty() {
                    self.comment_status = "No comments yet. (cached)".to_string();
                } else {
                    let total = self.comments.len();
                    let visible = self.visible_comment_indices.len();
                    if visible == total {
                        self.comment_status = format!("{total} comments loaded (cached)");
                    } else {
                        let hidden = total.saturating_sub(visible);
                        self.comment_status = format!(
                            "{total} comments loaded (cached) · {visible} visible · {hidden} hidden",
                        );
                    }
                }
                self.pending_comments = None;
                self.dismiss_link_menu(None);
                return Ok(());
            }
        }

        if let Some(pending) = self.pending_comments.take() {
            pending.cancel_flag.store(true, Ordering::SeqCst);
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let post_name = key.clone();

        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.pending_comments = Some(PendingComments {
            request_id,
            post_name: post_name.clone(),
            cancel_flag: cancel_flag.clone(),
        });
        self.comment_status = "Loading comments...".to_string();
        self.comments.clear();
        self.collapsed_comments.clear();
        self.visible_comment_indices.clear();
        self.spinner.reset();
        self.dismiss_link_menu(None);

        let tx = self.response_tx.clone();
        let service = service.clone();

        thread::spawn(move || {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let result = service.load_comments(&subreddit, &article).map(|listing| {
                let mut entries = Vec::new();
                collect_comments(&listing.comments, 0, &mut entries);
                entries
            });
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let _ = tx.send(AsyncResponse::Comments {
                request_id,
                post_name,
                result,
            });
        });
        Ok(())
    }

    fn draw(&mut self, frame: &mut Frame<'_>) {
        let full = frame.size();
        frame.render_widget(Block::default().style(Style::default().bg(COLOR_BG)), full);

        let layout = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(0),
                Constraint::Length(1),
            ])
            .split(frame.size());

        let status_text = if self.is_loading() {
            format!("{} {}", self.spinner.frame(), self.status_message)
                .trim()
                .to_string()
        } else {
            self.status_message.clone()
        };
        let status_line = Paragraph::new(status_text).style(
            Style::default()
                .fg(COLOR_TEXT_PRIMARY)
                .bg(COLOR_PANEL_FOCUSED_BG)
                .add_modifier(Modifier::BOLD),
        );
        frame.render_widget(status_line, layout[0]);

        let window = self.visible_panes();
        let constraints = pane_constraints(&window);
        let main_chunks = Layout::default()
            .direction(Direction::Horizontal)
            .constraints(constraints)
            .split(layout[1]);

        for (pane, area) in window.iter().zip(main_chunks.iter()) {
            match pane {
                Pane::Navigation => self.draw_subreddits(frame, *area),
                Pane::Posts => self.draw_posts(frame, *area),
                Pane::Content => self.draw_content(frame, *area),
                Pane::Comments => self.draw_comments(frame, *area),
            }
        }

        let footer = Paragraph::new(self.footer_text())
            .style(
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .bg(COLOR_PANEL_BG)
                    .add_modifier(Modifier::ITALIC),
            )
            .alignment(Alignment::Center)
            .wrap(Wrap { trim: true });
        frame.render_widget(footer, layout[2]);

        if self.menu_visible {
            self.draw_menu(frame, layout[1]);
        }

        if self.link_menu_visible {
            self.draw_link_menu(frame, layout[1]);
        }
    }

    fn flush_inline_images(&mut self, backend: &mut CrosstermBackend<Stdout>) -> Result<()> {
        self.flush_pending_kitty_deletes(backend)?;

        if self.link_menu_visible || self.menu_visible {
            self.needs_kitty_flush = true;
            self.emit_active_kitty_delete(backend)?;
            return Ok(());
        }

        if !self.needs_kitty_flush {
            return Ok(());
        }
        self.needs_kitty_flush = false;

        let mut requested_redraw = false;

        let Some(area) = self.content_area else {
            self.emit_active_kitty_delete(backend)?;
            return Ok(());
        };
        let Some(post) = self.posts.get(self.selected_post) else {
            self.emit_active_kitty_delete(backend)?;
            return Ok(());
        };
        let post_name = post.post.name.clone();

        if self
            .active_kitty
            .as_ref()
            .is_some_and(|active| active.post_name != post_name)
        {
            self.emit_active_kitty_delete(backend)?;
        }
        let Some(preview) = self.media_previews.get_mut(&post_name) else {
            if self.active_kitty_matches(&post_name) {
                self.emit_active_kitty_delete(backend)?;
            }
            return Ok(());
        };
        let Some(layout) = self.media_layouts.get(&post_name) else {
            if self.active_kitty_matches(&post_name) {
                self.emit_active_kitty_delete(backend)?;
            }
            return Ok(());
        };
        let Some(kitty) = preview.kitty_mut() else {
            if self.active_kitty_matches(&post_name) {
                self.emit_active_kitty_delete(backend)?;
            }
            return Ok(());
        };

        let content_width = area.width.saturating_sub(layout.indent).max(1);
        let lines = &self.content.lines;
        let line_offset = layout.line_offset.min(lines.len());
        let scroll_lines = self.content_scroll as usize;
        let visual_offset = visual_height(&lines[..line_offset], content_width);
        let visual_scroll = visual_height(&lines[..scroll_lines.min(lines.len())], content_width);

        if visual_offset < visual_scroll {
            if self.active_kitty_matches(&post_name) {
                self.emit_active_kitty_delete(backend)?;
            }
            return Ok(());
        }
        let relative_row = visual_offset - visual_scroll;
        if relative_row >= area.height as usize {
            if self.active_kitty_matches(&post_name) {
                self.emit_active_kitty_delete(backend)?;
            }
            return Ok(());
        }

        let row = area.y + relative_row as u16;
        let col = area.x.saturating_add(layout.indent);

        if kitty_debug_enabled() {
            eprintln!(
                "kitty_debug: post={} col={} row={} cols={} rows={} area=({},{} {}x{}) scroll={} line_offset={} indent={} content_scroll={}",
                post_name,
                col,
                row,
                kitty.cols,
                kitty.rows,
                area.x,
                area.y,
                area.width,
                area.height,
                visual_scroll,
                visual_offset,
                layout.indent,
                self.content_scroll
            );
        }

        let was_transmitted = kitty.transmitted;
        kitty.ensure_transmitted(backend)?;
        let sequence = kitty.placement_sequence();
        crossterm::queue!(backend, MoveTo(col, row), Print(sequence))?;
        backend.flush()?;

        if !was_transmitted {
            requested_redraw = true;
        }

        self.active_kitty = Some(ActiveKitty {
            post_name,
            image_id: kitty.id,
            wrap_tmux: kitty.wrap_tmux,
        });

        if requested_redraw {
            self.needs_redraw = true;
        }

        Ok(())
    }

    fn pane_block(&self, pane: Pane) -> Block<'static> {
        let focused = self.focused_pane == pane;
        let border_style = if focused {
            Style::default().fg(COLOR_BORDER_FOCUSED)
        } else {
            Style::default().fg(COLOR_BORDER_IDLE)
        };
        let title_style = if focused {
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_TEXT_SECONDARY)
        };
        Block::default()
            .title(Span::styled(pane.title(), title_style))
            .borders(Borders::ALL)
            .border_style(border_style)
            .style(Style::default().bg(COLOR_PANEL_BG))
            .padding(Padding::uniform(1))
    }

    fn draw_subreddits(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = self.pane_block(Pane::Navigation);
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let focused = self.focused_pane == Pane::Navigation;

        let layout_chunks = if inner.height <= 4 {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Length(2), Constraint::Min(0)])
                .split(inner)
        } else {
            Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Length(2),
                    Constraint::Length(2),
                    Constraint::Min(0),
                ])
                .split(inner)
        };
        let sort_area = layout_chunks[0];
        let instructions_area = if layout_chunks.len() > 2 {
            Some(layout_chunks[1])
        } else {
            None
        };
        let list_area = if instructions_area.is_some() {
            layout_chunks[2]
        } else {
            layout_chunks[1]
        };

        let mut sort_spans: Vec<Span> = Vec::with_capacity(NAV_SORTS.len() * 2);
        for (idx, sort) in NAV_SORTS.iter().enumerate() {
            if idx > 0 {
                sort_spans.push(Span::raw("  "));
            }
            let is_active = self.sort == *sort;
            let is_selected = focused && matches!(self.nav_mode, NavMode::Sorts);
            let mut style = Style::default().fg(if is_active {
                COLOR_ACCENT
            } else {
                COLOR_TEXT_SECONDARY
            });
            if is_selected {
                style = style
                    .add_modifier(Modifier::BOLD)
                    .bg(COLOR_PANEL_SELECTED_BG)
                    .fg(COLOR_TEXT_PRIMARY);
            }
            let marker = if is_active { "●" } else { "○" };
            let number = idx + 1;
            let label = format!("{} {} {}", number, marker, sort_label(*sort));
            sort_spans.push(Span::styled(label, style));
        }
        let sort_lines = vec![
            Line::from(vec![Span::styled(
                "Sort",
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .add_modifier(Modifier::BOLD),
            )]),
            Line::from(sort_spans),
        ];

        let sorts_paragraph = Paragraph::new(Text::from(sort_lines))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: false });
        frame.render_widget(sorts_paragraph, sort_area);

        if let Some(area) = instructions_area {
            let instructions = Paragraph::new(Text::from(vec![
                Line::from(vec![Span::styled(
                    "Controls",
                    Style::default()
                        .fg(COLOR_TEXT_PRIMARY)
                        .add_modifier(Modifier::BOLD),
                )]),
                Line::from(vec![Span::styled(
                    "h/l or ←/→ switch panes · j/k move within the list (press k on first row to reach sort) · digits/Enter load selection",
                    Style::default().fg(COLOR_TEXT_SECONDARY),
                )]),
            ]))
            .alignment(Alignment::Left)
            .wrap(Wrap { trim: true });
            frame.render_widget(instructions, area);
        }

        let width = list_area.width.max(1) as usize;
        let mut items: Vec<ListItem> = Vec::with_capacity(self.subreddits.len().max(1));
        for (idx, name) in self.subreddits.iter().enumerate() {
            let is_selected =
                focused && matches!(self.nav_mode, NavMode::Subreddits) && self.nav_index == idx;
            let is_active = self.selected_sub == idx;
            let background = if is_selected {
                COLOR_PANEL_SELECTED_BG
            } else {
                COLOR_PANEL_BG
            };
            let mut style = Style::default()
                .fg(if is_selected || is_active {
                    COLOR_TEXT_PRIMARY
                } else {
                    COLOR_TEXT_SECONDARY
                })
                .bg(background);
            if is_selected || is_active {
                style = style.add_modifier(Modifier::BOLD);
            }
            let mut lines = wrap_plain(name, width, style);
            lines.push(Line::from(Span::styled(
                String::new(),
                Style::default().bg(background),
            )));
            pad_lines_to_width(&mut lines, list_area.width);
            items.push(ListItem::new(lines));
        }

        if items.is_empty() {
            let mut lines = vec![Line::from(Span::styled(
                "No subreddits",
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .bg(COLOR_PANEL_BG)
                    .add_modifier(Modifier::ITALIC),
            ))];
            pad_lines_to_width(&mut lines, list_area.width);
            items.push(ListItem::new(lines));
        }

        let list = List::new(items);
        frame.render_widget(list, list_area);
    }

    fn draw_posts(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = self.pane_block(Pane::Posts);
        let inner = block.inner(area);
        let width = inner.width.max(1) as usize;
        let pane_width = inner.width;
        self.post_view_height.set(inner.height);
        self.ensure_post_visible();
        let offset = self.post_offset.get().min(self.posts.len());

        let mut score_width = 0usize;
        let mut comments_width = 0usize;
        for item in &self.posts {
            score_width = score_width.max(item.post.score.to_string().chars().count());
            comments_width = comments_width.max(item.post.num_comments.to_string().chars().count());
        }
        score_width = score_width.max(3);
        comments_width = comments_width.max(2);

        let loading_posts = self.pending_posts.is_some();
        let mut items: Vec<ListItem> = Vec::new();
        if offset == 0 {
            if let Some(update) = &self.update_notice {
                let message = format!("Update available: {} -> {} (GitHub Releases)",
                    self.current_version, update.version
                );
                let mut lines = vec![Line::from(Span::styled(
                    message,
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .bg(COLOR_PANEL_BG)
                        .add_modifier(Modifier::BOLD),
                ))];
                pad_lines_to_width(&mut lines, pane_width);
                items.push(ListItem::new(lines));
            }
        }
        let remaining_height = self.available_post_height(offset);

        self.prepare_post_rows(width, score_width, comments_width);

        if loading_posts && offset == 0 && !self.posts.is_empty() {
            let mut header_lines = Vec::new();
            header_lines.push(Line::from(Span::styled(
                format!("{} Loading new posts…", self.spinner.frame()),
                Style::default()
                    .fg(COLOR_ACCENT)
                    .bg(COLOR_PANEL_BG)
                    .add_modifier(Modifier::BOLD),
            )));
            header_lines.push(Line::from(Span::styled(
                String::new(),
                Style::default().bg(COLOR_PANEL_BG),
            )));
            pad_lines_to_width(&mut header_lines, pane_width);
            items.push(ListItem::new(header_lines));
        }

        let mut used_height = 0usize;
        for (idx, item) in self.posts.iter().enumerate().skip(offset) {
            let focused = self.focused_pane == Pane::Posts;
            let selected = idx == self.selected_post;
            let highlight = focused && selected;
            let background = if highlight {
                COLOR_PANEL_SELECTED_BG
            } else {
                COLOR_PANEL_BG
            };

            let primary_color = if highlight {
                COLOR_ACCENT
            } else if focused || selected {
                COLOR_TEXT_PRIMARY
            } else {
                COLOR_TEXT_SECONDARY
            };
            let identity_style = Style::default().fg(primary_color).bg(background);
            let mut title_style = Style::default()
                .fg(if focused {
                    COLOR_TEXT_PRIMARY
                } else {
                    COLOR_TEXT_SECONDARY
                })
                .bg(background);
            if selected && !focused {
                title_style = title_style.fg(COLOR_TEXT_PRIMARY);
            }
            if highlight {
                title_style = title_style.add_modifier(Modifier::BOLD);
            }
            let metrics_style = Style::default().fg(primary_color).bg(background);

            let post_name = &item.post.name;
            let mut push_item = |mut lines: Vec<Line<'static>>| {
                let item_height = lines.len().saturating_add(1).max(1);
                if remaining_height > 0
                    && used_height + item_height > remaining_height
                    && !items.is_empty()
                {
                    return false;
                }
                lines.push(Line::from(Span::styled(
                    String::new(),
                    Style::default().bg(background),
                )));
                pad_lines_to_width(&mut lines, pane_width);
                used_height = used_height.saturating_add(item_height.min(remaining_height));
                items.push(ListItem::new(lines));
                if remaining_height == 0 {
                    return false;
                }
                if remaining_height > 0 && used_height >= remaining_height {
                    return false;
                }
                true
            };

            if let Some(row) = self.post_rows.get(post_name) {
                let mut lines: Vec<Line<'static>> = Vec::new();
                let mut identity_lines = restyle_lines(&row.identity, identity_style);
                lines.append(&mut identity_lines);

                let mut title_lines = restyle_lines(&row.title, title_style);
                lines.append(&mut title_lines);

                let mut metrics_lines = restyle_lines(&row.metrics, metrics_style);
                lines.append(&mut metrics_lines);
                if !push_item(lines) {
                    break;
                }
            } else {
                let mut lines: Vec<Line<'static>> = Vec::new();
                lines.push(Line::from(Span::styled(
                    format!("{} Formatting post…", self.spinner.frame()),
                    Style::default()
                        .fg(if highlight || focused {
                            COLOR_TEXT_PRIMARY
                        } else {
                            COLOR_TEXT_SECONDARY
                        })
                        .bg(background),
                )));
                lines.push(Line::from(Span::styled(item.title.clone(), title_style)));
                if !push_item(lines) {
                    break;
                }
            }
        }

        if items.is_empty() {
            if loading_posts {
                let mut lines = vec![Line::from(Span::styled(
                    format!("{} Loading feed...", self.spinner.frame()),
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .bg(COLOR_PANEL_BG)
                        .add_modifier(Modifier::BOLD),
                ))];
                pad_lines_to_width(&mut lines, pane_width);
                items.push(ListItem::new(lines));
            } else {
                let mut lines = vec![Line::from(Span::styled(
                    "No posts loaded yet.",
                    Style::default()
                        .fg(COLOR_TEXT_SECONDARY)
                        .bg(COLOR_PANEL_BG)
                        .add_modifier(Modifier::ITALIC),
                ))];
                pad_lines_to_width(&mut lines, pane_width);
                items.push(ListItem::new(lines));
            }
        }

        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    fn prepare_post_rows(&mut self, width: usize, score_width: usize, comments_width: usize) {
        if width == 0 {
            return;
        }

        let width_changed = width != self.post_rows_width;
        if width_changed {
            self.post_rows.clear();
            self.pending_post_rows = None;
            self.post_rows_width = width;
        }

        if self.posts.is_empty() {
            self.pending_post_rows = None;
            return;
        }

        let mut inputs: Vec<PostRowInput> = Vec::new();
        for post in &self.posts {
            let name = post.post.name.clone();
            if !width_changed && self.post_rows.contains_key(&name) {
                continue;
            }
            inputs.push(PostRowInput {
                name,
                title: post.title.clone(),
                subreddit: post.post.subreddit.clone(),
                author: post.post.author.clone(),
                score: post.post.score,
                comments: post.post.num_comments,
                vote: match post.post.likes {
                    Some(true) => 1,
                    Some(false) => -1,
                    None => 0,
                },
            });
        }

        if inputs.is_empty() {
            return;
        }

        if let Some(pending) = &self.pending_post_rows {
            if pending.width == width {
                return;
            }
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        self.pending_post_rows = Some(PendingPostRows { request_id, width });

        let tx = self.response_tx.clone();
        thread::spawn(move || {
            let mut rows = Vec::with_capacity(inputs.len());
            for input in inputs {
                let data = build_post_row_data(&input, width, score_width, comments_width);
                rows.push((input.name, data));
            }
            let _ = tx.send(AsyncResponse::PostRows {
                request_id,
                width,
                rows,
            });
        });
    }

    fn compose_content(&mut self, base: Text<'static>, post: &PostPreview) -> Text<'static> {
        let key = post.post.name.clone();
        let mut lines = base.lines;
        self.media_layouts.remove(&key);
        if let Some(preview) = self.media_previews.get(&key) {
            if !lines.is_empty() && !line_is_blank(lines.last().unwrap()) {
                lines.push(Line::raw(String::new()));
            }
            let offset = lines.len();
            lines.extend(preview.placeholder().lines.clone());
            self.media_layouts.insert(
                key.clone(),
                MediaLayout {
                    line_offset: offset,
                    indent: MEDIA_INDENT,
                },
            );
            if preview.has_kitty() {
                self.needs_kitty_flush = true;
            }
        } else if !self.media_failures.contains(&key) {
            let mut offset = lines.len();
            if !lines.is_empty() && !line_is_blank(lines.last().unwrap()) {
                offset = lines.len();
                lines.push(Line::raw(String::new()));
            }
            lines.push(Line::from(Span::styled(
                "Loading preview...",
                Style::default().fg(COLOR_TEXT_SECONDARY),
            )));
            self.media_layouts.insert(
                key.clone(),
                MediaLayout {
                    line_offset: offset,
                    indent: MEDIA_INDENT,
                },
            );
        }
        Text {
            lines,
            alignment: base.alignment,
            style: base.style,
        }
    }

    fn queue_content_render(&mut self, post_name: String, source: String) {
        if let Some(pending) = self.pending_content.take() {
            pending.cancel_flag.store(true, Ordering::SeqCst);
        }

        let request_id = self.next_request_id;
        self.next_request_id = self.next_request_id.wrapping_add(1);
        let cancel_flag = Arc::new(AtomicBool::new(false));
        self.pending_content = Some(PendingContent {
            request_id,
            post_name: post_name.clone(),
            cancel_flag: cancel_flag.clone(),
        });

        let tx = self.response_tx.clone();
        thread::spawn(move || {
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let renderer = markdown::Renderer::new();
            let rendered = renderer.render(&source);
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            let _ = tx.send(AsyncResponse::Content {
                request_id,
                post_name,
                rendered,
            });
        });
    }

    fn draw_content(&mut self, frame: &mut Frame<'_>, area: Rect) {
        let block = self.pane_block(Pane::Content);
        let inner = block.inner(area);
        self.content_area = Some(inner);
        if self.selected_post_has_kitty_preview() {
            self.needs_kitty_flush = true;
        }
        let paragraph = Paragraph::new(self.content.clone())
            .block(block)
            .wrap(Wrap { trim: false })
            .scroll((self.content_scroll, 0));
        frame.render_widget(paragraph, area);
    }

    fn draw_comments(&self, frame: &mut Frame<'_>, area: Rect) {
        let block = self.pane_block(Pane::Comments);
        let inner = block.inner(area);
        let width = inner.width.max(1) as usize;
        let focused = self.focused_pane == Pane::Comments;
        self.comment_view_height.set(inner.height);
        self.comment_view_width.set(inner.width);
        let total_visible = self.visible_comment_indices.len();

        let comment_status = if self.pending_comments.is_some() {
            format!("{} {}", self.spinner.frame(), self.comment_status)
                .trim()
                .to_string()
        } else {
            self.comment_status.clone()
        };
        let status_style = Style::default()
            .fg(COLOR_TEXT_SECONDARY)
            .bg(COLOR_PANEL_BG)
            .add_modifier(Modifier::BOLD);
        let mut status_lines = wrap_plain(&comment_status, width, status_style);
        status_lines.push(Line::from(Span::styled(String::new(), status_style)));
        pad_lines_to_width(&mut status_lines, inner.width);
        self.comment_status_height.set(status_lines.len());
        self.ensure_comment_visible();
        let offset = self.comment_offset.get().min(total_visible);
        let available_height = self.available_comment_height();
        let mut used_height = 0usize;
        let mut items: Vec<ListItem> =
            Vec::with_capacity(total_visible.saturating_sub(offset).saturating_add(1));
        items.push(ListItem::new(status_lines));
        for (visible_idx, comment_index) in
            self.visible_comment_indices.iter().enumerate().skip(offset)
        {
            let comment = match self.comments.get(*comment_index) {
                Some(entry) => entry,
                None => continue,
            };
            let selected = visible_idx == self.selected_comment;
            let highlight = focused && selected;
            let background = if highlight {
                COLOR_PANEL_SELECTED_BG
            } else {
                COLOR_PANEL_BG
            };

            let mut meta_style = Style::default()
                .fg(comment_depth_color(comment.depth))
                .bg(background);
            if highlight {
                meta_style = meta_style.add_modifier(Modifier::BOLD);
            } else if focused {
                meta_style = meta_style.add_modifier(Modifier::ITALIC);
            }

            let body_color = if highlight || focused || selected {
                COLOR_TEXT_PRIMARY
            } else {
                COLOR_TEXT_SECONDARY
            };
            let body_style = Style::default().fg(body_color).bg(background);

            let collapsed = self.collapsed_comments.contains(comment_index);
            let indicator = if collapsed { "[+]" } else { "[-]" };

            let mut lines =
                comment_lines(comment, width, indicator, meta_style, body_style, collapsed);
            let item_height = lines.len().saturating_add(1);
            if available_height > 0
                && used_height > 0
                && used_height + item_height > available_height
            {
                break;
            }
            lines.push(Line::from(Span::styled(String::new(), body_style)));
            pad_lines_to_width(&mut lines, inner.width);
            items.push(ListItem::new(lines));
            if available_height == 0 {
                break;
            }
            if available_height > 0 {
                used_height = used_height.saturating_add(item_height.min(available_height));
                if used_height >= available_height {
                    break;
                }
            }
        }

        let list = List::new(items).block(block);
        frame.render_widget(list, area);
    }

    fn draw_menu(&self, frame: &mut Frame<'_>, area: Rect) {
        let popup_area = centered_rect(70, 70, area);
        frame.render_widget(Clear, popup_area);
        let menu = Paragraph::new(self.menu_body())
            .block(
                Block::default()
                    .title(Span::styled(
                        "Guided Menu",
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_ACCENT))
                    .style(Style::default().bg(COLOR_PANEL_BG)),
            )
            .wrap(Wrap { trim: false });
        frame.render_widget(menu, popup_area);
    }

    fn draw_link_menu(&self, frame: &mut Frame<'_>, area: Rect) {
        let popup_area = centered_rect(70, 60, area);
        frame.render_widget(Clear, popup_area);

        let mut items: Vec<ListItem> = Vec::new();
        if self.link_menu_items.is_empty() {
            items.push(ListItem::new(vec![Line::from(Span::styled(
                "No links available",
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .bg(COLOR_PANEL_BG)
                    .add_modifier(Modifier::ITALIC),
            ))]));
        } else {
            for entry in &self.link_menu_items {
                let lines = vec![
                    Line::from(Span::styled(
                        entry.label.clone(),
                        Style::default()
                            .fg(COLOR_TEXT_PRIMARY)
                            .bg(COLOR_PANEL_BG)
                            .add_modifier(Modifier::BOLD),
                    )),
                    Line::from(Span::styled(
                        entry.url.clone(),
                        Style::default().fg(COLOR_ACCENT).bg(COLOR_PANEL_BG),
                    )),
                    Line::default(),
                ];
                items.push(ListItem::new(lines));
            }
        }

        let list = List::new(items)
            .block(
                Block::default()
                    .title(Span::styled(
                        "Links",
                        Style::default()
                            .fg(COLOR_ACCENT)
                            .add_modifier(Modifier::BOLD),
                    ))
                    .borders(Borders::ALL)
                    .border_style(Style::default().fg(COLOR_ACCENT))
                    .style(Style::default().bg(COLOR_PANEL_BG)),
            )
            .highlight_style(
                Style::default()
                    .fg(COLOR_TEXT_PRIMARY)
                    .bg(COLOR_PANEL_SELECTED_BG)
                    .add_modifier(Modifier::BOLD),
            )
            .highlight_symbol("▶ ");

        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(1), Constraint::Length(1)])
            .split(popup_area);

        let mut state = ListState::default();
        if !self.link_menu_items.is_empty() {
            state.select(Some(
                self.link_menu_selected
                    .min(self.link_menu_items.len().saturating_sub(1)),
            ));
        }

        frame.render_stateful_widget(list, chunks[0], &mut state);

        let instructions = Paragraph::new("j/k move · Enter open · Esc/q close")
            .alignment(Alignment::Center)
            .style(
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .bg(COLOR_PANEL_BG)
                    .add_modifier(Modifier::ITALIC),
            );
        frame.render_widget(instructions, chunks[1]);
    }

    fn menu_field_line(&self, field: MenuField) -> Line<'static> {
        let is_active = self.menu_form.active == field;
        let mut spans = Vec::new();
        let indicator_style = if is_active {
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(COLOR_TEXT_SECONDARY)
        };
        spans.push(Span::styled(
            if is_active { ">" } else { " " }.to_string(),
            indicator_style,
        ));
        spans.push(Span::raw(" "));

        match field {
            MenuField::Save => {
                let button_style = if is_active {
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(COLOR_TEXT_SECONDARY)
                        .add_modifier(Modifier::BOLD)
                };
                spans.push(Span::styled("[ Save & Close ]".to_string(), button_style));
                spans.push(Span::raw("  Press Enter to write credentials"));
            }
            MenuField::CopyLink => {
                let button_style = if is_active {
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    Style::default()
                        .fg(COLOR_TEXT_SECONDARY)
                        .add_modifier(Modifier::BOLD)
                };
                let label = if self.menu_form.auth_pending {
                    "[ Copy Link ]  Waiting for redirect… press Enter or c to copy".to_string()
                } else {
                    "[ Copy Link ]  Press Enter or c to copy URL again".to_string()
                };
                spans.push(Span::styled(label, button_style));
            }
            _ => {
                let label_style = if is_active {
                    Style::default()
                        .fg(COLOR_ACCENT)
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default()
                        .fg(COLOR_TEXT_SECONDARY)
                        .add_modifier(Modifier::BOLD)
                };
                spans.push(Span::styled(field.title().to_string(), label_style));
                spans.push(Span::raw(": "));

                let display = self.menu_form.display_value(field);
                let value_style = if display == "(not set)" {
                    Style::default().fg(COLOR_TEXT_SECONDARY)
                } else if is_active {
                    Style::default().fg(COLOR_ACCENT)
                } else {
                    Style::default().fg(COLOR_TEXT_PRIMARY)
                };
                spans.push(Span::styled(display, value_style));
            }
        }

        Line::from(spans)
    }

    fn menu_body(&self) -> Text<'static> {
        match self.menu_screen {
            MenuScreen::Accounts => self.menu_accounts_body(),
            MenuScreen::Credentials => self.menu_credentials_body(),
        }
    }

    fn menu_accounts_body(&self) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Account Manager".to_string(),
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::default());

        if self.menu_accounts.is_empty() {
            lines.push(Line::from(vec![Span::styled(
                "No Reddit accounts saved.".to_string(),
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .add_modifier(Modifier::ITALIC),
            )]));
        } else {
            for (idx, entry) in self.menu_accounts.iter().enumerate() {
                let selected = self.menu_account_index == idx;
                let indicator_style = Style::default().fg(if selected {
                    COLOR_ACCENT
                } else {
                    COLOR_TEXT_SECONDARY
                });
                let mut label_style = Style::default().fg(if selected {
                    COLOR_TEXT_PRIMARY
                } else {
                    COLOR_TEXT_SECONDARY
                });
                if selected {
                    label_style = label_style.add_modifier(Modifier::BOLD);
                }
                if entry.is_active {
                    label_style = label_style.add_modifier(Modifier::UNDERLINED);
                }
                let mut display = entry.display.clone();
                if entry.is_active {
                    display.push_str(" (active)");
                }
                lines.push(Line::from(vec![
                    Span::styled(
                        if selected { ">" } else { " " }.to_string(),
                        indicator_style,
                    ),
                    Span::raw(" "),
                    Span::styled(display, label_style),
                ]));
            }
        }

        let positions = self.menu_account_positions();

        let add_selected = self.menu_account_index == positions.add;
        let mut add_style = Style::default().fg(if add_selected {
            COLOR_ACCENT
        } else {
            COLOR_TEXT_SECONDARY
        });
        if add_selected {
            add_style = add_style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            Span::styled(if add_selected { ">" } else { " " }.to_string(), add_style),
            Span::raw(" "),
            Span::styled("Add new account…".to_string(), add_style),
        ]));

        lines.push(Line::default());
        lines.push(Line::from(vec![Span::styled(
            "Stay in the loop with the community:".to_string(),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        )]));
        let join_index = positions.join;
        let join_selected = self.menu_account_index == join_index;
        let join_state = self.active_join_state();
        let label = if join_state.is_some_and(|state| state.pending) {
            "[ Joining r/ReddixTUI… ]"
        } else if join_state.is_some_and(|state| state.joined) {
            "[ Joined r/ReddixTUI ]"
        } else {
            "[ Join r/ReddixTUI ]"
        };
        let joined = join_state.is_some_and(|state| state.joined);
        let join_indicator_style = Style::default().fg(if join_selected {
            COLOR_ACCENT
        } else if joined {
            COLOR_SUCCESS
        } else {
            COLOR_TEXT_SECONDARY
        });
        let mut join_label_style = Style::default().fg(if joined {
            COLOR_SUCCESS
        } else if join_selected {
            COLOR_ACCENT
        } else {
            COLOR_TEXT_SECONDARY
        });
        if join_selected && !joined {
            join_label_style = join_label_style.add_modifier(Modifier::BOLD | Modifier::REVERSED);
        } else {
            join_label_style = join_label_style.add_modifier(Modifier::BOLD);
        }
        lines.push(Line::from(vec![
            Span::styled(
                if join_selected { ">" } else { " " }.to_string(),
                join_indicator_style,
            ),
            Span::raw(" "),
            Span::styled(label.to_string(), join_label_style),
        ]));

        let (join_hint, join_hint_style) = match (join_state, self.active_account_id()) {
            (Some(state), _) if state.last_error.is_some() => (
                state.last_error.clone().unwrap(),
                Style::default().fg(COLOR_ERROR),
            ),
            (Some(state), _) if state.pending => (
                "Request sent… hang tight.".to_string(),
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .add_modifier(Modifier::ITALIC),
            ),
            (Some(state), _) if state.joined => (
                "Already subscribed. Thanks for supporting the community!".to_string(),
                Style::default().fg(COLOR_SUCCESS),
            ),
            (_, Some(_)) => (
                "Press Enter to subscribe using your active account.".to_string(),
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .add_modifier(Modifier::ITALIC),
            ),
            _ => (
                "Add an account to enable one-click subscribe.".to_string(),
                Style::default()
                    .fg(COLOR_TEXT_SECONDARY)
                    .add_modifier(Modifier::ITALIC),
            ),
        };
        lines.push(Line::from(vec![Span::styled(join_hint, join_hint_style)]));

        let github_index = positions.github;
        let support_index = positions.support;

        let github_selected = self.menu_account_index == github_index;
        let github_indicator_style = Style::default().fg(if github_selected {
            COLOR_ACCENT
        } else {
            COLOR_TEXT_SECONDARY
        });
        let mut github_label_style = Style::default().fg(if github_selected {
            COLOR_ACCENT
        } else {
            COLOR_TEXT_SECONDARY
        });
        if github_selected {
            github_label_style = github_label_style.add_modifier(Modifier::BOLD);
        }

        let support_selected = self.menu_account_index == support_index;
        let support_indicator_style = Style::default().fg(if support_selected {
            COLOR_ACCENT
        } else {
            COLOR_TEXT_SECONDARY
        });
        let mut support_label_style = Style::default().fg(if support_selected {
            COLOR_ACCENT
        } else {
            COLOR_TEXT_SECONDARY
        });
        if support_selected {
            support_label_style = support_label_style.add_modifier(Modifier::BOLD);
        }

        lines.push(Line::default());
        lines.push(Line::from(vec![
            Span::styled(
                if github_selected { ">" } else { " " }.to_string(),
                github_indicator_style,
            ),
            Span::raw(" "),
            Span::styled(
                "Check the project out on GitHub · ".to_string(),
                github_label_style,
            ),
            Span::styled(
                PROJECT_LINK_URL.to_string(),
                Style::default().fg(COLOR_ACCENT),
            ),
        ]));
        lines.push(Line::from(vec![
            Span::styled(
                if support_selected { ">" } else { " " }.to_string(),
                support_indicator_style,
            ),
            Span::raw(" "),
            Span::styled(
                "Support the project (opens browser) · ".to_string(),
                support_label_style,
            ),
            Span::styled(
                SUPPORT_LINK_URL.to_string(),
                Style::default().fg(COLOR_ACCENT),
            ),
        ]));
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::styled(
            "Controls: j/k select · Enter switch/select · a add account · Esc/m close".to_string(),
            Style::default().fg(COLOR_TEXT_SECONDARY),
        )]));

        Text::from(lines)
    }

    fn menu_credentials_body(&self) -> Text<'static> {
        let mut lines: Vec<Line<'static>> = Vec::new();
        lines.push(Line::from(vec![Span::styled(
            "Setup & Login Guide".to_string(),
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]));
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::raw(
            "1. Open Reddit app preferences at https://www.reddit.com/prefs/apps and create a script app."
                .to_string(),
        )]));
        lines.push(Line::from(vec![Span::raw(
            "2. Add the local redirect URI 127.0.0.1:65010/reddix/callback as authorized."
                .to_string(),
        )]));
        lines.push(Line::from(vec![Span::raw(format!(
            "3. Reddix will update {} with your credentials.",
            self.config_path
        ))]));
        lines.push(Line::from(vec![Span::raw(
            "4. After saving, press r in the main view to reload Reddit data.".to_string(),
        )]));
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::styled(
            "Credentials".to_string(),
            Style::default()
                .fg(COLOR_ACCENT)
                .add_modifier(Modifier::BOLD),
        )]));
        let mut fields = vec![
            MenuField::ClientId,
            MenuField::ClientSecret,
            MenuField::UserAgent,
            MenuField::Save,
        ];
        if self.menu_form.has_auth_link() {
            fields.push(MenuField::CopyLink);
        }
        for field in fields {
            lines.push(self.menu_field_line(field));
        }
        if self.menu_form.auth_url.is_some() {
            lines.push(Line::default());
            lines.push(Line::from(vec![Span::styled(
                "Authorization Link".to_string(),
                Style::default()
                    .fg(COLOR_ACCENT)
                    .add_modifier(Modifier::BOLD),
            )]));
            let message = if self.menu_form.auth_pending {
                "Link ready (press c to copy)".to_string()
            } else {
                "Press c to copy the authorization link".to_string()
            };
            lines.push(Line::from(vec![Span::styled(
                message,
                Style::default().fg(COLOR_ACCENT),
            )]));
            if self.menu_form.auth_pending {
                lines.push(Line::from(vec![Span::raw(
                    "Waiting for Reddit to redirect back to Reddix...".to_string(),
                )]));
            }
        }
        lines.push(Line::default());
        lines.push(Line::from(vec![Span::raw(
            "Controls: Tab/Shift-Tab or Up/Down to move | Backspace/Delete to edit | Enter to advance/save/copy | c to copy link | Esc back | m close".
                to_string(),
        )]));
        if let Some(status) = &self.menu_form.status {
            lines.push(Line::default());
            let lowered = status.to_lowercase();
            let style = if lowered.contains("fail") || lowered.contains("error") {
                Style::default().fg(COLOR_ERROR)
            } else {
                Style::default().fg(COLOR_SUCCESS)
            };
            lines.push(Line::from(vec![Span::styled(status.clone(), style)]));
        }
        Text::from(lines)
    }

    fn footer_text(&self) -> String {
        if self.menu_visible {
            return match self.menu_screen {
                MenuScreen::Accounts => {
                    "Guided menu: j/k select account · Enter switch · a add account · Esc/m close"
                        .to_string()
                }
                MenuScreen::Credentials => {
                    "Guided menu: Tab/Shift-Tab change field · Enter save/advance · c copy link · Esc back · m close"
                        .to_string()
                }
            };
        }

        let mut parts: Vec<String> = Vec::new();
        parts.push("Links menu (o)".to_string());

        match self.focused_pane {
            Pane::Navigation => match self.nav_mode {
                NavMode::Sorts => {
                    parts.push("Sorts: ←/→ or 1-5 change order".to_string());
                    parts.push("Enter reloads feed".to_string());
                    if !self.subreddits.is_empty() {
                        parts.push("Press j to jump to subscriptions".to_string());
                    }
                    parts.push("s refresh subscriptions".to_string());
                }
                NavMode::Subreddits => {
                    parts.push("Subreddits: j/k move, Enter load".to_string());
                    parts.push("k on first returns to sorts".to_string());
                    parts.push("s refresh subscriptions".to_string());
                }
            },
            Pane::Posts => {
                if self.posts.is_empty() {
                    parts.push("Posts: waiting for feed…".to_string());
                } else {
                    parts.push("Posts: j/k move, digits jump, Space/Page scroll".to_string());
                    parts.push("Votes: u upvote, d downvote".to_string());
                }
            }
            Pane::Content => {
                parts.push("Content: ↑/↓ scroll, PageUp/PageDown faster".to_string());
                if !self.posts.is_empty() {
                    parts.push("Votes: u upvote, d downvote".to_string());
                }
            }
            Pane::Comments => {
                if self.pending_comments.is_some() {
                    parts.push("Loading comments…".to_string());
                } else if self.comments.is_empty() {
                    parts.push("No comments yet".to_string());
                } else {
                    parts.push("Comments: j/k move, c fold, Shift+C expand".to_string());
                    parts.push("Votes: u upvote, d downvote".to_string());
                }
            }
        }

        if self.pending_posts.is_some() {
            parts.push("Refreshing feed…".to_string());
        }

        parts.push("r refresh posts".to_string());
        parts.push("m guided menu".to_string());
        parts.push("q quit".to_string());

        parts.join(" · ")
    }
}

fn pane_constraints(panes: &[Pane; 3]) -> [Constraint; 3] {
    match panes {
        [Pane::Navigation, Pane::Posts, Pane::Content] => [
            Constraint::Percentage(20),
            Constraint::Percentage(45),
            Constraint::Percentage(35),
        ],
        [Pane::Posts, Pane::Content, Pane::Comments] => [
            Constraint::Percentage(30),
            Constraint::Percentage(25),
            Constraint::Percentage(45),
        ],
        [Pane::Navigation, Pane::Posts, Pane::Comments] => [
            Constraint::Percentage(20),
            Constraint::Percentage(35),
            Constraint::Percentage(45),
        ],
        [Pane::Navigation, Pane::Content, Pane::Comments] => [
            Constraint::Percentage(20),
            Constraint::Percentage(30),
            Constraint::Percentage(50),
        ],
        _ => [
            Constraint::Percentage(30),
            Constraint::Percentage(30),
            Constraint::Percentage(40),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn total_width(line: &Line<'_>) -> usize {
        line.spans
            .iter()
            .map(|span| UnicodeWidthStr::width(span.content.as_ref()))
            .sum()
    }

    #[test]
    fn pad_lines_extends_to_width() {
        let mut lines = vec![Line::from(vec![Span::raw("abc")])];
        pad_lines_to_width(&mut lines, 6);
        assert_eq!(lines[0].spans.len(), 2);
        assert_eq!(lines[0].spans[1].content.as_ref(), "   ");
        assert_eq!(total_width(&lines[0]), 6);
    }

    #[test]
    fn pad_lines_does_not_shorten() {
        let mut lines = vec![Line::from(vec![Span::raw("abcdef")])];
        pad_lines_to_width(&mut lines, 4);
        assert_eq!(lines[0].spans.len(), 1);
        assert_eq!(total_width(&lines[0]), 6);
    }

    #[test]
    fn pad_lines_supports_wide_glyphs() {
        let mut lines = vec![Line::from(vec![Span::raw("🦀")])];
        pad_lines_to_width(&mut lines, 3);
        assert_eq!(total_width(&lines[0]), 3);
        assert_eq!(lines[0].spans.len(), 2);
    }

    #[test]
    fn indent_media_preview_unchanged_when_indent_zero() {
        let preview = "line one\nline two";
        assert_eq!(indent_media_preview(preview), preview);
    }

    #[test]
    fn kitty_placeholder_matches_dimensions() {
        let placeholder = kitty_placeholder_text(4, 2, 0, "example");
        assert_eq!(placeholder.lines.len(), 3);
        assert_eq!(placeholder.lines[0].spans[0].content.as_ref(), "    ");
        assert_eq!(
            placeholder.lines[2].spans[0].content.as_ref(),
            "[image: example]"
        );
    }
}
