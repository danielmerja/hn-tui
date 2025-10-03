use anyhow::{Context, Result};
use rand::seq::SliceRandom;
use std::sync::Arc;

use crate::reddit::{self, ListingOptions, SortOption};

pub trait SubredditService: Send + Sync {
    fn list_subreddits(&self, source: reddit::SubredditSource) -> Result<Vec<reddit::Subreddit>>;
}

pub trait FeedService: Send + Sync {
    fn load_front_page(&self, sort: SortOption) -> Result<reddit::Listing<reddit::Post>>;
    fn load_subreddit(&self, name: &str, sort: SortOption)
        -> Result<reddit::Listing<reddit::Post>>;
}

pub trait CommentService: Send + Sync {
    fn load_comments(&self, subreddit: &str, article: &str) -> Result<reddit::PostComments>;
}

pub trait InteractionService: Send + Sync {
    fn vote(&self, fullname: &str, dir: i32) -> Result<()>;
    fn save(&self, fullname: &str, category: Option<&str>) -> Result<()>;
    fn unsave(&self, fullname: &str) -> Result<()>;
    fn hide(&self, fullname: &str) -> Result<()>;
    fn unhide(&self, fullname: &str) -> Result<()>;
    fn reply(&self, parent: &str, text: &str) -> Result<reddit::Comment>;
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
        let listing = self
            .client
            .subreddits(source, ListingOptions::default())
            .context("fetch subreddit listing")?;
        Ok(listing
            .children
            .into_iter()
            .map(|thing| thing.data)
            .collect())
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
    fn load_front_page(&self, sort: SortOption) -> Result<reddit::Listing<reddit::Post>> {
        self.client
            .front_page(sort, ListingOptions::default())
            .context("fetch front page")
    }

    fn load_subreddit(
        &self,
        name: &str,
        sort: SortOption,
    ) -> Result<reddit::Listing<reddit::Post>> {
        self.client
            .subreddit_listing(name, sort, ListingOptions::default())
            .context("fetch subreddit feed")
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
    fn load_comments(&self, subreddit: &str, article: &str) -> Result<reddit::PostComments> {
        self.client
            .comments(subreddit, article, ListingOptions::default())
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
    fn load_front_page(&self, _sort: SortOption) -> Result<reddit::Listing<reddit::Post>> {
        Ok(mock_listing("Welcome to Reddix"))
    }

    fn load_subreddit(
        &self,
        name: &str,
        _sort: SortOption,
    ) -> Result<reddit::Listing<reddit::Post>> {
        Ok(mock_listing(&format!("Sample posts for {}", name)))
    }
}

#[derive(Default)]
pub struct MockCommentService;

impl CommentService for MockCommentService {
    fn load_comments(&self, subreddit: &str, article: &str) -> Result<reddit::PostComments> {
        Ok(reddit::PostComments {
            post: reddit::Post {
                id: article.into(),
                name: article.into(),
                title: format!("{} — {}", subreddit, article),
                subreddit: subreddit.into(),
                author: "reddix".into(),
                selftext: "Comments are unavailable in this mock response.".into(),
                url: String::new(),
                permalink: format!("/{}/{}", subreddit, article),
                score: 1,
                likes: None,
                num_comments: 0,
                created_utc: 0.0,
                thumbnail: String::new(),
                stickied: false,
                over_18: false,
                spoiler: false,
                post_hint: String::new(),
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

    fn reply(&self, _parent: &str, _text: &str) -> Result<reddit::Comment> {
        Ok(reddit::Comment {
            id: "mock".into(),
            name: "mock".into(),
            body: "Thanks for trying Reddix!".into(),
            author: "reddix".into(),
            score: 1,
            likes: None,
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
