//! Inbox categories: Primary, Promotions and Social, as tabs over the Inbox.
//!
//! Each message gets a category when it's downloaded, guessed from its
//! headers (Gmail accounts use Gmail's own categories instead, see
//! `engine`). A conversation's category is the lowest of its messages', so
//! anything with a reply in it is Primary. Sender rules ("always put mail
//! from this address in Promotions") override the guess; they're kept on
//! the server with the snooze list, so every OwlPost agrees.

use mail_parser::{Address, Message};

pub const PRIMARY: u8 = 0;
pub const PROMOTIONS: u8 = 1;
pub const SOCIAL: u8 = 2;
/// Every category, in tab order.
pub const ALL: [u8; 3] = [PRIMARY, PROMOTIONS, SOCIAL];

/// The name used in queries and the MCP tools.
pub fn name(c: u8) -> &'static str {
    match c {
        PROMOTIONS => "promotions",
        SOCIAL => "social",
        _ => "primary",
    }
}

/// The tab label.
pub fn label(c: u8) -> &'static str {
    match c {
        PROMOTIONS => "Promotions",
        SOCIAL => "Social",
        _ => "Primary",
    }
}

/// A category by name, any case.
pub fn parse(s: &str) -> Option<u8> {
    ALL.into_iter().find(|c| name(*c).eq_ignore_ascii_case(s.trim()))
}

/// Social networks: their notifications go to Social.
const SOCIAL_DOMAINS: &[&str] = &[
    "facebookmail.com", "facebook.com", "instagram.com", "linkedin.com", "twitter.com", "x.com", "tiktok.com",
    "pinterest.com", "redditmail.com", "reddit.com", "threads.net", "snapchat.com", "tumblr.com", "nextdoor.com",
    "meetup.com", "strava.com", "discord.com", "quora.com", "goodreads.com", "flickr.com", "bsky.app", "medium.com",
];

/// Mailing tools that only marketing mail comes from (as the X-Mailer).
const MARKETING_MAILERS: &[&str] = &[
    "mailchimp", "klaviyo", "hubspot", "sendinblue", "brevo", "mailerlite", "campaign monitor", "createsend",
    "constant contact", "marketo", "exacttarget", "salesforce marketing", "braze", "iterable", "customer.io",
    "omnisend", "activecampaign", "convertkit", "kit.com", "substack", "beehiiv",
];

/// Headers that only marketing platforms add.
const MARKETING_HEADERS: &[&str] = &[
    "X-MC-User", "X-Mailchimp-Campaign", "X-Campaign", "X-CampaignID", "X-Campaign-Id", "X-CSA-Complaints",
    "X-SFMC-Stack", "X-Klaviyo-Message", "X-Kmail-Relay", "X-HS-Cid", "X-Mailin-Campaign", "X-Mailer-LID",
    "X-Marketo-Id", "X-Braze-Campaign", "X-ConvertKit", "X-Substack-Post",
];

/// The domain of an address, lower-case.
fn domain(email: &str) -> String {
    email.rsplit_once('@').map(|(_, d)| d.trim().trim_end_matches('>').to_ascii_lowercase()).unwrap_or_default()
}

/// Is this domain (or a subdomain of it) in the list?
fn in_domains(d: &str, list: &[&str]) -> bool {
    list.iter().any(|x| d == *x || d.ends_with(&format!(".{x}")))
}

/// Guess a message's category from its headers.
pub fn classify(msg: &Message) -> u8 {
    let from = match msg.from() {
        Some(Address::List(l)) => l.first().and_then(|a| a.address()).unwrap_or_default().to_string(),
        Some(Address::Group(g)) => g.first().and_then(|g| g.addresses.first()).and_then(|a| a.address()).unwrap_or_default().to_string(),
        None => String::new(),
    };
    if in_domains(&domain(&from), SOCIAL_DOMAINS) {
        return SOCIAL;
    }
    let has = |h: &'static str| msg.header_raw(h).is_some();
    // Replies, and discussion lists (and notification lists like GitHub's,
    // which publish an archive), are conversations.
    if has("In-Reply-To") || has("List-Post") || has("List-Archive") {
        return PRIMARY;
    }
    if !has("List-Unsubscribe") {
        return PRIMARY;
    }
    let bulk = msg.header_raw("Precedence").is_some_and(|v| v.trim().eq_ignore_ascii_case("bulk"));
    let mailer = msg.header_raw("X-Mailer").map(|m| m.to_ascii_lowercase()).unwrap_or_default();
    let marketing = MARKETING_HEADERS.iter().any(|h| has(h)) || MARKETING_MAILERS.iter().any(|m| mailer.contains(m));
    // One-click unsubscribe (RFC 8058) is what bulk senders must offer.
    if bulk || marketing || has("List-Unsubscribe-Post") {
        PROMOTIONS
    } else {
        PRIMARY
    }
}

/// [`classify`] from raw bytes (a whole message or just its header).
pub fn classify_raw(raw: &[u8]) -> u8 {
    mail_parser::MessageParser::default().parse_headers(raw).map(|m| classify(&m)).unwrap_or(PRIMARY)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn c(headers: &str) -> u8 {
        classify_raw(format!("{headers}\r\nSubject: hi\r\n\r\nbody\r\n").as_bytes())
    }

    #[test]
    fn sorts_by_headers() {
        // A person.
        assert_eq!(c("From: Maya <maya@example.com>"), PRIMARY);
        // A shop's newsletter.
        assert_eq!(c("From: Kowhai <hello@kowhai.co.nz>\r\nList-Unsubscribe: <https://k.co/u>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click"), PROMOTIONS);
        assert_eq!(c("From: Shop <news@shop.com>\r\nList-Unsubscribe: <mailto:u@shop.com>\r\nX-Mailer: Klaviyo"), PROMOTIONS);
        assert_eq!(c("From: Shop <news@shop.com>\r\nList-Unsubscribe: <mailto:u@shop.com>\r\nPrecedence: bulk"), PROMOTIONS);
        // Unsubscribe link alone (a club, a receipt): still Primary.
        assert_eq!(c("From: Club <club@running.org>\r\nList-Unsubscribe: <mailto:u@running.org>"), PRIMARY);
        // GitHub-style notification lists publish an archive.
        assert_eq!(c("From: GitHub <notifications@github.com>\r\nList-Archive: https://github.com/x\r\nList-Unsubscribe: <mailto:u@github.com>\r\nList-Unsubscribe-Post: List-Unsubscribe=One-Click"), PRIMARY);
        // Social networks, subdomains included.
        assert_eq!(c("From: LinkedIn <messages-noreply@linkedin.com>"), SOCIAL);
        assert_eq!(c("From: Facebook <notification@facebookmail.com>\r\nList-Unsubscribe: <https://f.com/u>\r\nList-Unsubscribe-Post: x"), SOCIAL);
        assert_eq!(c("From: Strava <no-reply@mail.strava.com>"), SOCIAL);
        // Replies are conversations.
        assert_eq!(c("From: Shop <news@shop.com>\r\nIn-Reply-To: <a@b>\r\nList-Unsubscribe: <mailto:u@shop.com>\r\nPrecedence: bulk"), PRIMARY);
    }

    #[test]
    fn names() {
        assert_eq!(parse("Promotions"), Some(PROMOTIONS));
        assert_eq!(parse("nope"), None);
        assert_eq!(label(SOCIAL), "Social");
    }
}
