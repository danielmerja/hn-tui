use std::sync::Arc;

use anyhow::{Context, Result};

use crate::config;
use crate::data::{self, CommentService, FeedService, InteractionService, SubredditService};
use crate::hackernews;
use crate::media;
use crate::reddit;
use crate::session;
use crate::storage;
use crate::ui;

pub fn run() -> Result<()> {
    let cfg = config::load(config::LoadOptions::default()).context("load config")?;
    let config_path = config::default_path();
    let display_path = friendly_path(config_path.as_ref());

    let store =
        Arc::new(storage::Store::open(storage::Options::default()).context("open storage")?);

    let media_cfg = media::Config {
        cache_dir: cfg.media.cache_dir.clone(),
        max_size_bytes: cfg.media.max_size_bytes,
        default_ttl: cfg.media.default_ttl,
        workers: cfg.media.workers,
        http_client: None,
        max_queue_depth: cfg.media.max_queue_depth,
    };
    let media_manager = media::Manager::new(store.clone(), media_cfg).ok();
    let media_handle = media_manager.as_ref().map(|manager| manager.handle());

    let _theme = &cfg.ui.theme;
    let status: String;
    let content: String;
    
    let subreddits = vec![
        "Top".to_string(),
        "New".to_string(),
        "Best".to_string(),
        "Ask HN".to_string(),
        "Show HN".to_string(),
        "Jobs".to_string(),
    ];
    let mut posts: Vec<ui::PostPreview> = vec![
        placeholder_post(
            "welcome",
            "Welcome to HN-TUI",
            "Browse Hacker News from your terminal.\n\nUse j/k to navigate stories, Enter to view comments, h/l to switch panes.",
        ),
        placeholder_post(
            "shortcuts",
            "Keyboard shortcuts",
            "j/k: Navigate up/down\nh/l: Switch between panes\nEnter: View story or comments\np: Refresh\nq: Quit",
        ),
    ];

    let mut feed_service: Option<Arc<dyn data::FeedService + Send + Sync>> = None;
    let mut subreddit_service: Option<Arc<dyn data::SubredditService + Send + Sync>> = None;
    let mut comment_service: Option<Arc<dyn data::CommentService + Send + Sync>> = None;
    let mut interaction_service: Option<Arc<dyn data::InteractionService + Send + Sync>> = None;

    let session_manager: Option<Arc<session::Manager>> = None;
    let fetch_subreddits_on_start = true;

    // Initialize HackerNews client (no authentication needed)
    let user_agent = if !cfg.reddit.user_agent.trim().is_empty() {
        cfg.reddit.user_agent.clone()
    } else {
        format!("hn-tui/{}", crate::VERSION)
    };

    if let Ok(client) = hackernews::Client::new(hackernews::ClientConfig {
        user_agent: user_agent.clone(),
        http_client: None,
    }) {
        let client = Arc::new(client);
        
        // Create HackerNews service implementations
        let subreddit_api: Arc<dyn SubredditService + Send + Sync> =
            Arc::new(data::HackerNewsCategoryService::new(client.clone()));
        let feed_api: Arc<dyn FeedService + Send + Sync> =
            Arc::new(data::HackerNewsFeedService::new(client.clone()));
        let comment_api: Arc<dyn CommentService + Send + Sync> =
            Arc::new(data::HackerNewsCommentService::new(client.clone()));
        let interaction_api: Arc<dyn InteractionService + Send + Sync> =
            Arc::new(data::HackerNewsInteractionService::new());

        feed_service = Some(feed_api);
        subreddit_service = Some(subreddit_api);
        comment_service = Some(comment_api);
        interaction_service = Some(interaction_api);
        
        status = "Browsing Hacker News. Press j/k to navigate, Enter to view comments, q to quit.".to_string();
        content = "HN-TUI is ready! Select a category on the left and browse stories.\n\nNo authentication required - all HN content is public.".to_string();
        posts.clear();
    } else {
        status = "Failed to initialize HackerNews client.".to_string();
        content = "Could not connect to Hacker News. Please check your internet connection.".to_string();
    }

    let options = ui::Options {
        status_message: status,
        subreddits,
        posts,
        content,
        feed_service,
        subreddit_service,
        default_sort: reddit::SortOption::Hot,
        default_comment_sort: reddit::CommentSortOption::Confidence,
        comment_service,
        interaction_service,
        media_handle,
        config_path: display_path.clone(),
        store: store.clone(),
        session_manager: session_manager.clone(),
        fetch_subreddits_on_start,
    };

    let mut model = ui::Model::new(options);
    model.run()?;

    if let Some(manager) = session_manager {
        manager.close();
    }
    drop(media_manager);

    Ok(())
}

fn friendly_path(path: Option<&std::path::PathBuf>) -> String {
    if let Some(path) = path {
        if let Some(home) = dirs::home_dir() {
            if let Ok(stripped) = path.strip_prefix(&home) {
                let mut display = String::from("~");
                if !stripped.as_os_str().is_empty() {
                    display.push_str(&format!("/{}", stripped.display()));
                }
                return display;
            }
        }
        path.display().to_string()
    } else {
        "~/.config/hn-tui/config.yaml".to_string()
    }
}

fn placeholder_post(id: &str, title: &str, description: &str) -> ui::PostPreview {
    let body = format!("{title}\n\n{description}");
    let links = Vec::new();
    ui::PostPreview {
        title: title.to_string(),
        body,
        post: reddit::Post {
            id: id.to_string(),
            name: format!("s_{id}"),
            title: title.to_string(),
            subreddit: "top".to_string(),
            author: "hn-tui".to_string(),
            selftext: description.to_string(),
            url: String::new(),
            permalink: format!("/item/{id}"),
            score: 0,
            likes: None,
            num_comments: 0,
            created_utc: 0.0,
            thumbnail: String::new(),
            stickied: false,
            over_18: false,
            spoiler: false,
            post_hint: String::new(),
            is_video: false,
            media: None,
            secure_media: None,
            crosspost_parent_list: Vec::new(),
            preview: reddit::Preview::default(),
            gallery_data: None,
            media_metadata: None,
        },
        links,
    }
}
