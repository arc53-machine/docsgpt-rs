//! A bot's agents and how a message picks one.

use std::collections::HashSet;

use crate::config::{AgentConfig, normalize_name};
use crate::error::{Error, Result};

/// The validated agents of one bot. Never empty; exactly one is the default.
#[derive(Debug, Clone)]
pub struct Agents {
    list: Vec<AgentConfig>,
}

/// The agent a message is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Routed<'a> {
    /// Ask `agent` the `question`.
    Agent {
        /// The chosen agent.
        agent: &'a AgentConfig,
        /// The message with any `#name` prefix removed.
        question: String,
        /// True when the message named the agent with `#name`.
        tagged: bool,
    },
    /// The message started with `#tag` that names no agent, and the bot has several.
    UnknownTag {
        /// The tag as typed, without `#`.
        tag: String,
        /// Names of the available agents.
        available: Vec<String>,
    },
}

impl Agents {
    /// Validate and normalize: names lowercased and unique, keys non-empty, at
    /// most one default (the first agent becomes the default when none is marked).
    pub fn new(mut list: Vec<AgentConfig>) -> Result<Self> {
        if list.is_empty() {
            return Err(Error::config("at least one agent is required"));
        }
        let mut seen = HashSet::new();
        for a in &mut list {
            a.name = normalize_name("agent", &a.name)?;
            if !seen.insert(a.name.clone()) {
                return Err(Error::config(format!("duplicate agent {:?}", a.name)));
            }
            if a.api_key.trim().is_empty() {
                return Err(Error::config(format!("agent {:?} has an empty api_key", a.name)));
            }
        }
        match list.iter().filter(|a| a.default).count() {
            0 => list[0].default = true,
            1 => {}
            _ => return Err(Error::config("more than one default agent")),
        }
        Ok(Self { list })
    }

    /// The default agent.
    pub fn default_agent(&self) -> &AgentConfig {
        self.list.iter().find(|a| a.default).unwrap_or(&self.list[0])
    }

    /// An agent by name (case-insensitive).
    pub fn get(&self, name: &str) -> Option<&AgentConfig> {
        self.list.iter().find(|a| a.name.eq_ignore_ascii_case(name))
    }

    /// All agents, in configuration order.
    pub fn iter(&self) -> impl Iterator<Item = &AgentConfig> {
        self.list.iter()
    }

    /// Number of agents.
    pub fn len(&self) -> usize {
        self.list.len()
    }

    /// Always false; kept for API symmetry with `len`.
    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    /// Route a message: a leading `#name` picks that agent for this message;
    /// otherwise the chat's `active` agent (if it still exists), else the default.
    ///
    /// A `#tag` that names no agent is an error only when the bot has several
    /// agents; with one agent the message is asked as typed (it may be a hashtag).
    pub fn route<'a>(&'a self, text: &str, active: Option<&str>) -> Routed<'a> {
        let trimmed = text.trim();
        if let Some(rest) = trimmed.strip_prefix('#') {
            let (tag, remainder) = match rest.find(char::is_whitespace) {
                Some(i) => (&rest[..i], rest[i..].trim()),
                None => (rest, ""),
            };
            if let Some(agent) = self.get(tag) {
                return Routed::Agent {
                    agent,
                    question: remainder.to_string(),
                    tagged: true,
                };
            }
            if self.len() > 1 && !tag.is_empty() {
                return Routed::UnknownTag {
                    tag: tag.to_string(),
                    available: self.list.iter().map(|a| a.name.clone()).collect(),
                };
            }
        }
        let agent = active.and_then(|n| self.get(n)).unwrap_or_else(|| self.default_agent());
        Routed::Agent {
            agent,
            question: trimmed.to_string(),
            tagged: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agents(names: &[&str]) -> Agents {
        Agents::new(names.iter().map(|n| AgentConfig::new(*n, format!("key-{n}"))).collect()).unwrap()
    }

    fn agent_of(r: Routed<'_>) -> (String, String, bool) {
        match r {
            Routed::Agent {
                agent,
                question,
                tagged,
            } => (agent.name.clone(), question, tagged),
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn validates() {
        assert!(Agents::new(vec![]).is_err());
        assert!(Agents::new(vec![AgentConfig::new("a", "k"), AgentConfig::new("A", "k2")]).is_err());
        assert!(Agents::new(vec![AgentConfig::new("a", " ")]).is_err());
        let two_defaults = vec![
            AgentConfig {
                default: true,
                ..AgentConfig::new("a", "k")
            },
            AgentConfig {
                default: true,
                ..AgentConfig::new("b", "k")
            },
        ];
        assert!(Agents::new(two_defaults).is_err());
        let a = Agents::new(vec![
            AgentConfig::new("Support", "k"),
            AgentConfig {
                default: true,
                ..AgentConfig::new("b", "k")
            },
        ])
        .unwrap();
        assert_eq!(a.default_agent().name, "b");
        assert_eq!(a.get("SUPPORT").unwrap().name, "support");
        assert_eq!(agents(&["x", "y"]).default_agent().name, "x");
    }

    #[test]
    fn routes_by_tag_active_and_default() {
        let a = agents(&["support", "sales"]);
        assert_eq!(
            agent_of(a.route("  #Sales  what's the price? ", None)),
            ("sales".into(), "what's the price?".into(), true)
        );
        assert_eq!(agent_of(a.route("#sales", None)), ("sales".into(), "".into(), true));
        assert_eq!(
            agent_of(a.route("hello", Some("sales"))),
            ("sales".into(), "hello".into(), false)
        );
        assert_eq!(
            agent_of(a.route("hello", Some("gone"))),
            ("support".into(), "hello".into(), false)
        );
        assert_eq!(
            a.route("#nope hi", None),
            Routed::UnknownTag {
                tag: "nope".into(),
                available: vec!["support".into(), "sales".into()]
            }
        );
        // A bare "#" is not a tag.
        assert_eq!(agent_of(a.route("# heading", None)).0, "support");
    }

    #[test]
    fn single_agent_keeps_hashtags() {
        let a = agents(&["docs"]);
        assert_eq!(
            agent_of(a.route("#rust is great", None)),
            ("docs".into(), "#rust is great".into(), false)
        );
    }
}
