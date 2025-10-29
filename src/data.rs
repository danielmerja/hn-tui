use anyhow::{Context, Result};
use rand::seq::SliceRandom;
use std::collections::HashSet;
use std::sync::Arc;

use crate::hackernews;
use crate::reddit::{self, CommentSortOption, ListingOptions, SortOption};

pub trait SubredditService: Send + Sync {
    fn list_subreddits(&self, source: reddit::SubredditSource) -> Result<Vec<reddit::Subreddit>>;
}

pub trait FeedService: Send + Sync {
    fn load_front_page(
        &self,
        sort: SortOption,
        opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>>;
    fn load_subreddit(
        &self,
        name: &str,
        sort: SortOption,
        opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>>;
    fn load_user(
        &self,
        name: &str,
        sort: SortOption,
        opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>>;
    fn search_posts(
        &self,
        query: &str,
        sort: SortOption,
        opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>>;
}

pub trait CommentService: Send + Sync {
    fn load_comments(
        &self,
        subreddit: &str,
        article: &str,
        sort: CommentSortOption,
    ) -> Result<reddit::PostComments>;
}

pub trait InteractionService: Send + Sync {
    fn vote(&self, fullname: &str, dir: i32) -> Result<()>;
    fn save(&self, fullname: &str, category: Option<&str>) -> Result<()>;
    fn unsave(&self, fullname: &str) -> Result<()>;
    fn hide(&self, fullname: &str) -> Result<()>;
    fn unhide(&self, fullname: &str) -> Result<()>;
    fn reply(&self, parent: &str, text: &str) -> Result<reddit::Comment>;
    fn subscribe(&self, subreddit: &str) -> Result<()>;
    fn is_subscribed(&self, subreddit: &str) -> Result<bool>;
}

pub struct RedditSubredditService {
    client: Arc<reddit::Client>,
}

impl RedditSubredditService {
    pub fn new(client: Arc<reddit::Client>) -> Self {
        Self { client }
    }
}

impl SubredditService for RedditSubredditService {
    fn list_subreddits(&self, source: reddit::SubredditSource) -> Result<Vec<reddit::Subreddit>> {
        const PER_PAGE: u32 = 100;
        const MAX_PAGES: usize = 100;

        let mut all = Vec::new();
        let mut seen = HashSet::new();
        let mut after: Option<String> = None;

        for _ in 0..MAX_PAGES {
            let opts = ListingOptions {
                limit: Some(PER_PAGE),
                after: after.clone(),
                ..Default::default()
            };

            let listing = self
                .client
                .subreddits(source, opts)
                .context("fetch subreddit listing page")?;

            let next_after = listing.after.clone();

            for thing in listing.children {
                let subreddit = thing.data;
                if seen.insert(subreddit.id.clone()) {
                    all.push(subreddit);
                }
            }

            if next_after.is_none() || next_after == after {
                break;
            }

            after = next_after;
        }

        Ok(all)
    }
}

pub struct RedditFeedService {
    client: Arc<reddit::Client>,
}

impl RedditFeedService {
    pub fn new(client: Arc<reddit::Client>) -> Self {
        Self { client }
    }
}

impl FeedService for RedditFeedService {
    fn load_front_page(
        &self,
        sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        self.client
            .front_page(sort, opts)
            .context("fetch front page")
    }

    fn load_subreddit(
        &self,
        name: &str,
        sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        self.client
            .subreddit_listing(name, sort, opts)
            .context("fetch subreddit feed")
    }

    fn load_user(
        &self,
        name: &str,
        sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        self.client
            .user_listing(name, sort, opts)
            .context("fetch user submissions")
    }

    fn search_posts(
        &self,
        query: &str,
        sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        self.client
            .search_posts(query, sort, opts)
            .context("search reddit")
    }
}

pub struct RedditCommentService {
    client: Arc<reddit::Client>,
}

impl RedditCommentService {
    pub fn new(client: Arc<reddit::Client>) -> Self {
        Self { client }
    }
}

impl CommentService for RedditCommentService {
    fn load_comments(
        &self,
        subreddit: &str,
        article: &str,
        sort: CommentSortOption,
    ) -> Result<reddit::PostComments> {
        self.client
            .comments(subreddit, article, sort, ListingOptions::default())
            .context("fetch comments")
    }
}

pub struct RedditInteractionService {
    client: Arc<reddit::Client>,
}

impl RedditInteractionService {
    pub fn new(client: Arc<reddit::Client>) -> Self {
        Self { client }
    }
}

impl InteractionService for RedditInteractionService {
    fn vote(&self, fullname: &str, dir: i32) -> Result<()> {
        self.client.vote(fullname, dir)
    }

    fn save(&self, fullname: &str, category: Option<&str>) -> Result<()> {
        self.client.save(fullname, category)
    }

    fn unsave(&self, fullname: &str) -> Result<()> {
        self.client.unsave(fullname)
    }

    fn hide(&self, fullname: &str) -> Result<()> {
        self.client.hide(fullname)
    }

    fn unhide(&self, fullname: &str) -> Result<()> {
        self.client.unhide(fullname)
    }

    fn subscribe(&self, subreddit: &str) -> Result<()> {
        self.client.subscribe_subreddit(subreddit)
    }

    fn is_subscribed(&self, subreddit: &str) -> Result<bool> {
        self.client.is_subscribed(subreddit)
    }

    fn reply(&self, parent: &str, text: &str) -> Result<reddit::Comment> {
        self.client.reply(parent, text)
    }
}

#[derive(Default)]
pub struct MockSubredditService;

impl SubredditService for MockSubredditService {
    fn list_subreddits(&self, _source: reddit::SubredditSource) -> Result<Vec<reddit::Subreddit>> {
        Ok(vec![
            reddit::Subreddit {
                id: "frontpage".into(),
                name: "r/frontpage".into(),
                title: "Frontpage".into(),
                subscribers: 0,
                over_18: false,
            },
            reddit::Subreddit {
                id: "popular".into(),
                name: "r/popular".into(),
                title: "Popular".into(),
                subscribers: 0,
                over_18: false,
            },
        ])
    }
}

#[derive(Default)]
pub struct MockFeedService;

impl FeedService for MockFeedService {
    fn load_front_page(
        &self,
        _sort: SortOption,
        _opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        Ok(mock_listing("Welcome to Reddix"))
    }

    fn load_subreddit(
        &self,
        name: &str,
        _sort: SortOption,
        _opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        Ok(mock_listing(&format!("Sample posts for {name}")))
    }

    fn load_user(
        &self,
        name: &str,
        _sort: SortOption,
        _opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        Ok(mock_listing(&format!("User posts for u/{name}")))
    }

    fn search_posts(
        &self,
        query: &str,
        _sort: SortOption,
        _opts: reddit::ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        Ok(mock_listing(&format!("Search results for {query}")))
    }
}

#[derive(Default)]
pub struct MockCommentService;

impl CommentService for MockCommentService {
    fn load_comments(
        &self,
        subreddit: &str,
        article: &str,
        _sort: CommentSortOption,
    ) -> Result<reddit::PostComments> {
        Ok(reddit::PostComments {
            post: reddit::Post {
                id: article.into(),
                name: article.into(),
                title: format!("{subreddit} — {article}"),
                subreddit: subreddit.into(),
                author: "reddix".into(),
                selftext: "Comments are unavailable in this mock response.".into(),
                url: String::new(),
                permalink: format!("/{subreddit}/{article}"),
                score: 1,
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
            comments: reddit::Listing {
                after: None,
                before: None,
                children: vec![],
            },
        })
    }
}

#[derive(Default)]
pub struct MockInteractionService;

impl InteractionService for MockInteractionService {
    fn vote(&self, _fullname: &str, _dir: i32) -> Result<()> {
        Ok(())
    }

    fn save(&self, _fullname: &str, _category: Option<&str>) -> Result<()> {
        Ok(())
    }

    fn unsave(&self, _fullname: &str) -> Result<()> {
        Ok(())
    }

    fn hide(&self, _fullname: &str) -> Result<()> {
        Ok(())
    }

    fn unhide(&self, _fullname: &str) -> Result<()> {
        Ok(())
    }

    fn subscribe(&self, _subreddit: &str) -> Result<()> {
        Ok(())
    }

    fn is_subscribed(&self, _subreddit: &str) -> Result<bool> {
        Ok(false)
    }

    fn reply(&self, _parent: &str, _text: &str) -> Result<reddit::Comment> {
        Ok(reddit::Comment {
            id: "mock".into(),
            name: "mock".into(),
            body: "Thanks for trying Reddix!".into(),
            author: "reddix".into(),
            score: 1,
            likes: None,
            score_hidden: false,
            depth: 0,
            created_utc: 0.0,
            replies: None,
        })
    }
}

fn mock_listing(title: &str) -> reddit::Listing<reddit::Post> {
    let mut rng = rand::thread_rng();
    let mut posts = vec![reddit::Post {
        id: "welcome".into(),
        name: "t3_welcome".into(),
        title: title.into(),
        subreddit: "r/reddix".into(),
        author: "team".into(),
        selftext: "Sample content provided for offline browsing.".into(),
        url: String::new(),
        permalink: "/r/reddix/welcome".into(),
        score: 1234,
        likes: None,
        num_comments: 42,
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
    }];

    posts.shuffle(&mut rng);

    reddit::Listing {
        after: None,
        before: None,
        children: posts
            .into_iter()
            .map(|post| reddit::Thing {
                kind: "t3".into(),
                data: post,
            })
            .collect(),
    }
}

pub fn sort_option_from_key(key: &str) -> SortOption {
    match key {
        "best" => SortOption::Best,
        "new" => SortOption::New,
        "top" => SortOption::Top,
        "rising" => SortOption::Rising,
        _ => SortOption::Hot,
    }
}

// HackerNews service implementations
pub struct HackerNewsCategoryService {
    _client: Arc<hackernews::Client>,
}

impl HackerNewsCategoryService {
    pub fn new(client: Arc<hackernews::Client>) -> Self {
        Self { _client: client }
    }
}

impl SubredditService for HackerNewsCategoryService {
    fn list_subreddits(&self, _source: reddit::SubredditSource) -> Result<Vec<reddit::Subreddit>> {
        // Return HN categories as "subreddits"
        Ok(vec![
            reddit::Subreddit {
                id: "top".into(),
                name: "Top".into(),
                title: "Top Stories".into(),
                subscribers: 0,
                over_18: false,
            },
            reddit::Subreddit {
                id: "new".into(),
                name: "New".into(),
                title: "New Stories".into(),
                subscribers: 0,
                over_18: false,
            },
            reddit::Subreddit {
                id: "best".into(),
                name: "Best".into(),
                title: "Best Stories".into(),
                subscribers: 0,
                over_18: false,
            },
            reddit::Subreddit {
                id: "ask".into(),
                name: "Ask HN".into(),
                title: "Ask HN".into(),
                subscribers: 0,
                over_18: false,
            },
            reddit::Subreddit {
                id: "show".into(),
                name: "Show HN".into(),
                title: "Show HN".into(),
                subscribers: 0,
                over_18: false,
            },
            reddit::Subreddit {
                id: "jobs".into(),
                name: "Jobs".into(),
                title: "HN Jobs".into(),
                subscribers: 0,
                over_18: false,
            },
        ])
    }
}

pub struct HackerNewsFeedService {
    client: Arc<hackernews::Client>,
}

impl HackerNewsFeedService {
    pub fn new(client: Arc<hackernews::Client>) -> Self {
        Self { client }
    }
}

impl FeedService for HackerNewsFeedService {
    fn load_front_page(
        &self,
        _sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        let start = opts.after.as_ref().and_then(|s| s.parse().ok()).unwrap_or(0);
        let limit = opts.limit.unwrap_or(30) as usize;
        
        let hn_listing = self.client
            .story_listing(hackernews::StoryType::Top, start, limit)
            .context("fetch HN top stories")?;
        
        // Convert HN stories to Reddit posts for compatibility
        Ok(reddit::Listing {
            after: hn_listing.after,
            before: hn_listing.before,
            children: hn_listing.children.into_iter().map(|thing| {
                reddit::Thing {
                    kind: thing.kind,
                    data: hn_story_to_reddit_post(thing.data),
                }
            }).collect(),
        })
    }

    fn load_subreddit(
        &self,
        name: &str,
        _sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        let start = opts.after.as_ref().and_then(|s| s.parse().ok()).unwrap_or(0);
        let limit = opts.limit.unwrap_or(30) as usize;
        
        let story_type = match name.to_lowercase().trim_start_matches("r/") {
            "new" => hackernews::StoryType::New,
            "best" => hackernews::StoryType::Best,
            "ask" | "askhn" => hackernews::StoryType::Ask,
            "show" | "showhn" => hackernews::StoryType::Show,
            "jobs" | "job" => hackernews::StoryType::Job,
            _ => hackernews::StoryType::Top,
        };
        
        let hn_listing = self.client
            .story_listing(story_type, start, limit)
            .context("fetch HN stories")?;
        
        Ok(reddit::Listing {
            after: hn_listing.after,
            before: hn_listing.before,
            children: hn_listing.children.into_iter().map(|thing| {
                reddit::Thing {
                    kind: thing.kind,
                    data: hn_story_to_reddit_post(thing.data),
                }
            }).collect(),
        })
    }

    fn load_user(
        &self,
        name: &str,
        _sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        let start = opts.after.as_ref().and_then(|s| s.parse().ok()).unwrap_or(0);
        let limit = opts.limit.unwrap_or(30) as usize;
        
        let hn_listing = self.client
            .user_stories(name, start, limit)
            .context("fetch HN user stories")?;
        
        Ok(reddit::Listing {
            after: hn_listing.after,
            before: hn_listing.before,
            children: hn_listing.children.into_iter().map(|thing| {
                reddit::Thing {
                    kind: thing.kind,
                    data: hn_story_to_reddit_post(thing.data),
                }
            }).collect(),
        })
    }

    fn search_posts(
        &self,
        query: &str,
        _sort: SortOption,
        opts: ListingOptions,
    ) -> Result<reddit::Listing<reddit::Post>> {
        let start = opts.after.as_ref().and_then(|s| s.parse().ok()).unwrap_or(0);
        let limit = opts.limit.unwrap_or(30) as usize;
        
        let hn_listing = self.client
            .search_stories(query, start, limit)
            .context("search HN")?;
        
        Ok(reddit::Listing {
            after: hn_listing.after,
            before: hn_listing.before,
            children: hn_listing.children.into_iter().map(|thing| {
                reddit::Thing {
                    kind: thing.kind,
                    data: hn_story_to_reddit_post(thing.data),
                }
            }).collect(),
        })
    }
}

pub struct HackerNewsCommentService {
    client: Arc<hackernews::Client>,
}

impl HackerNewsCommentService {
    pub fn new(client: Arc<hackernews::Client>) -> Self {
        Self { client }
    }
}

impl CommentService for HackerNewsCommentService {
    fn load_comments(
        &self,
        _subreddit: &str,
        article: &str,
        _sort: CommentSortOption,
    ) -> Result<reddit::PostComments> {
        let story_id: i64 = article.parse()
            .context("parse story ID")?;
        
        let hn_comments = self.client
            .comments(story_id)
            .context("fetch HN comments")?;
        
        Ok(reddit::PostComments {
            post: hn_story_to_reddit_post(hn_comments.story),
            comments: reddit::Listing {
                after: hn_comments.comments.after,
                before: hn_comments.comments.before,
                children: hn_comments.comments.children.into_iter().map(|thing| {
                    reddit::Thing {
                        kind: thing.kind,
                        data: hn_comment_to_reddit_comment(thing.data),
                    }
                }).collect(),
            },
        })
    }
}

pub struct HackerNewsInteractionService;

impl HackerNewsInteractionService {
    pub fn new() -> Self {
        Self
    }
}

impl InteractionService for HackerNewsInteractionService {
    fn vote(&self, _fullname: &str, _dir: i32) -> Result<()> {
        // HN API doesn't support voting
        Ok(())
    }

    fn save(&self, _fullname: &str, _category: Option<&str>) -> Result<()> {
        // HN API doesn't support saving
        Ok(())
    }

    fn unsave(&self, _fullname: &str) -> Result<()> {
        // HN API doesn't support unsaving
        Ok(())
    }

    fn hide(&self, _fullname: &str) -> Result<()> {
        // HN API doesn't support hiding
        Ok(())
    }

    fn unhide(&self, _fullname: &str) -> Result<()> {
        // HN API doesn't support unhiding
        Ok(())
    }

    fn subscribe(&self, _subreddit: &str) -> Result<()> {
        // HN doesn't have subscriptions
        Ok(())
    }

    fn is_subscribed(&self, _subreddit: &str) -> Result<bool> {
        // HN doesn't have subscriptions
        Ok(false)
    }

    fn reply(&self, _parent: &str, _text: &str) -> Result<reddit::Comment> {
        // HN API doesn't support posting comments
        anyhow::bail!("Posting comments is not supported via HN API")
    }
}

fn hn_story_to_reddit_post(story: hackernews::Story) -> reddit::Post {
    // Story is already a reddit::Post, so just return it
    story
}

fn hn_comment_to_reddit_comment(comment: reddit::Comment) -> reddit::Comment {
    // Comment is already a reddit::Comment, so just return it
    comment
}
