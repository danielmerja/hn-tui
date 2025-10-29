use std::time::Duration;

use anyhow::{anyhow, bail, Result};
use reqwest::blocking::Client as HttpClient;
use reqwest::header::USER_AGENT;
use serde::{Deserialize, Serialize};

use crate::reddit::{Comment, Listing, Thing};

pub const HN_API_BASE: &str = "https://hacker-news.firebaseio.com/v0";
pub const HN_ITEM_URL: &str = "https://news.ycombinator.com/item";

#[derive(Debug, Clone, Default)]
pub struct ClientConfig {
    pub user_agent: String,
    pub http_client: Option<HttpClient>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum StoryType {
    #[default]
    Top,
    New,
    Best,
    Ask,
    Show,
    Job,
}

impl StoryType {
    pub fn as_str(&self) -> &'static str {
        match self {
            StoryType::Top => "topstories",
            StoryType::New => "newstories",
            StoryType::Best => "beststories",
            StoryType::Ask => "askstories",
            StoryType::Show => "showstories",
            StoryType::Job => "jobstories",
        }
    }

    pub fn display_name(&self) -> &'static str {
        match self {
            StoryType::Top => "Top",
            StoryType::New => "New",
            StoryType::Best => "Best",
            StoryType::Ask => "Ask HN",
            StoryType::Show => "Show HN",
            StoryType::Job => "Jobs",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Hash, Default)]
#[serde(rename_all = "lowercase")]
pub enum CommentSortOption {
    #[default]
    Default,
}

impl CommentSortOption {
    pub fn as_str(&self) -> &'static str {
        "default"
    }
}

pub struct Client {
    http: HttpClient,
    user_agent: String,
    base_url: String,
}

impl Client {
    pub fn new(config: ClientConfig) -> Result<Self> {
        if config.user_agent.trim().is_empty() {
            bail!("hackernews client user agent required");
        }
        
        let http = match config.http_client {
            Some(client) => client,
            None => HttpClient::builder()
                .timeout(Duration::from_secs(20))
                .build()?,
        };

        Ok(Client {
            http,
            user_agent: config.user_agent,
            base_url: HN_API_BASE.to_string(),
        })
    }

    pub fn story_listing(&self, story_type: StoryType, start: usize, limit: usize) -> Result<Listing<Story>> {
        // First get the list of story IDs
        let url = format!("{}/{}.json", self.base_url, story_type.as_str());
        let ids: Vec<i64> = self.http
            .get(&url)
            .header(USER_AGENT, &self.user_agent)
            .send()?
            .json()?;

        // Fetch stories in the requested range
        let end = std::cmp::min(start + limit, ids.len());
        let mut stories = Vec::new();
        
        for id in &ids[start..end] {
            if let Ok(item) = self.get_item(*id) {
                if let Some(story) = item.into_story() {
                    stories.push(Thing {
                        kind: "story".to_string(),
                        data: story,
                    });
                }
            }
        }

        Ok(Listing {
            after: if end < ids.len() { Some(end.to_string()) } else { None },
            before: if start > 0 { Some(start.to_string()) } else { None },
            children: stories,
        })
    }

    pub fn user_stories(&self, username: &str, start: usize, limit: usize) -> Result<Listing<Story>> {
        let user = self.get_user(username)?;
        
        let end = std::cmp::min(start + limit, user.submitted.len());
        let mut stories = Vec::new();
        
        for id in &user.submitted[start..end] {
            if let Ok(item) = self.get_item(*id) {
                if let Some(story) = item.into_story() {
                    stories.push(Thing {
                        kind: "story".to_string(),
                        data: story,
                    });
                }
            }
        }

        Ok(Listing {
            after: if end < user.submitted.len() { Some(end.to_string()) } else { None },
            before: if start > 0 { Some(start.to_string()) } else { None },
            children: stories,
        })
    }

    pub fn search_stories(&self, _query: &str, _start: usize, _limit: usize) -> Result<Listing<Story>> {
        // Note: HN Firebase API doesn't support search
        // We would need to use Algolia API for this
        bail!("Search is not yet implemented for Hacker News");
    }

    pub fn comments(&self, story_id: i64) -> Result<StoryComments> {
        let item = self.get_item(story_id)?;
        
        let story = item.clone().into_story()
            .ok_or_else(|| anyhow!("Item {} is not a story", story_id))?;
        
        let mut comments = Vec::new();
        if let Some(kids) = &item.kids {
            for kid_id in kids {
                if let Ok(comment_item) = self.get_item(*kid_id) {
                    if let Some(comment) = self.item_to_comment(&comment_item, 0) {
                        comments.push(Thing {
                            kind: "comment".to_string(),
                            data: comment,
                        });
                    }
                }
            }
        }

        Ok(StoryComments {
            story,
            comments: Listing {
                after: None,
                before: None,
                children: comments,
            },
        })
    }

    fn item_to_comment(&self, item: &Item, depth: i64) -> Option<Comment> {
        if item.item_type != "comment" {
            return None;
        }

        let mut replies = Vec::new();
        if let Some(kids) = &item.kids {
            for kid_id in kids {
                if let Ok(kid_item) = self.get_item(*kid_id) {
                    if let Some(reply) = self.item_to_comment(&kid_item, depth + 1) {
                        replies.push(Thing {
                            kind: "comment".to_string(),
                            data: reply,
                        });
                    }
                }
            }
        }

        Some(Comment {
            id: item.id.to_string(),
            name: format!("c_{}", item.id),
            body: item.text.clone().unwrap_or_default(),
            author: item.by.clone().unwrap_or_default(),
            score: item.score.unwrap_or(0),
            likes: None,
            score_hidden: false,
            depth,
            created_utc: item.time.unwrap_or(0) as f64,
            replies: if replies.is_empty() {
                None
            } else {
                Some(Box::new(Listing {
                    after: None,
                    before: None,
                    children: replies,
                }))
            },
        })
    }

    fn get_item(&self, id: i64) -> Result<Item> {
        let url = format!("{}/item/{}.json", self.base_url, id);
        let item: Item = self.http
            .get(&url)
            .header(USER_AGENT, &self.user_agent)
            .send()?
            .json()?;
        Ok(item)
    }

    fn get_user(&self, username: &str) -> Result<User> {
        let url = format!("{}/user/{}.json", self.base_url, username);
        let user: User = self.http
            .get(&url)
            .header(USER_AGENT, &self.user_agent)
            .send()?
            .json()?;
        Ok(user)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Item {
    pub id: i64,
    #[serde(rename = "type")]
    pub item_type: String,
    #[serde(default)]
    pub by: Option<String>,
    #[serde(default)]
    pub time: Option<i64>,
    #[serde(default)]
    pub text: Option<String>,
    #[serde(default)]
    pub dead: bool,
    #[serde(default)]
    pub deleted: bool,
    #[serde(default)]
    pub parent: Option<i64>,
    #[serde(default)]
    pub kids: Option<Vec<i64>>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub score: Option<i64>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub descendants: Option<i64>,
}

impl Item {
    pub fn into_story(self) -> Option<Story> {
        if self.item_type != "story" && self.item_type != "job" {
            return None;
        }

        let title = self.title.clone().unwrap_or_default();
        let subreddit = match self.item_type.as_str() {
            "job" => "jobs".to_string(),
            _ => {
                let title_lower = title.to_lowercase();
                if title_lower.starts_with("ask hn") {
                    "ask".to_string()
                } else if title_lower.starts_with("show hn") {
                    "show".to_string()
                } else {
                    "top".to_string()
                }
            }
        };

        Some(Story {
            id: self.id.to_string(),
            name: format!("s_{}", self.id),
            title,
            subreddit,
            author: self.by.unwrap_or_default(),
            selftext: self.text.unwrap_or_default(),
            url: self.url.unwrap_or_default(),
            permalink: format!("{}?id={}", HN_ITEM_URL, self.id),
            score: self.score.unwrap_or(0),
            likes: None,
            num_comments: self.descendants.unwrap_or(0),
            created_utc: self.time.unwrap_or(0) as f64,
            thumbnail: String::new(),
            stickied: false,
            over_18: false,
            spoiler: false,
            post_hint: String::new(),
            is_video: false,
            media: None,
            secure_media: None,
            crosspost_parent_list: vec![],
            preview: Default::default(),
            gallery_data: None,
            media_metadata: None,
        })
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub created: i64,
    pub karma: i64,
    #[serde(default)]
    pub about: Option<String>,
    #[serde(default)]
    pub submitted: Vec<i64>,
}

// Re-export Story as alias for reddit::Post for compatibility
pub use crate::reddit::Post as Story;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoryComments {
    pub story: Story,
    pub comments: Listing<Comment>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Category {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub subscribers: i64,
    #[serde(default, rename = "over18")]
    pub over_18: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum CategorySource {
    All,
}

impl CategorySource {
    pub fn as_path(&self) -> &'static str {
        match self {
            CategorySource::All => "/categories",
        }
    }
}
