//! Known provider settings, so most people only type an email and password.

use crate::models::{Security, ServerConfig};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Provider {
    Gmail,
    ICloud,
    Outlook,
    Yahoo,
    Fastmail,
    Custom,
}

impl Provider {
    pub const ALL: [Provider; 6] = [
        Provider::Gmail,
        Provider::ICloud,
        Provider::Outlook,
        Provider::Yahoo,
        Provider::Fastmail,
        Provider::Custom,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Provider::Gmail => "Gmail / Google Workspace",
            Provider::ICloud => "iCloud Mail",
            Provider::Outlook => "Outlook.com / Office 365",
            Provider::Yahoo => "Yahoo Mail",
            Provider::Fastmail => "Fastmail",
            Provider::Custom => "IMAP / SMTP (other)",
        }
    }

    /// Guess the provider from the email domain.
    pub fn detect(email: &str) -> Provider {
        let domain = email.rsplit('@').next().unwrap_or("").to_ascii_lowercase();
        match domain.as_str() {
            "gmail.com" | "googlemail.com" => Provider::Gmail,
            "icloud.com" | "me.com" | "mac.com" => Provider::ICloud,
            "outlook.com" | "hotmail.com" | "live.com" | "msn.com" => Provider::Outlook,
            "yahoo.com" | "ymail.com" | "rocketmail.com" => Provider::Yahoo,
            "fastmail.com" | "fastmail.fm" => Provider::Fastmail,
            _ => Provider::Custom,
        }
    }

    pub fn imap(self) -> Option<ServerConfig> {
        let (host, port, security) = match self {
            Provider::Gmail => ("imap.gmail.com", 993, Security::Tls),
            Provider::ICloud => ("imap.mail.me.com", 993, Security::Tls),
            Provider::Outlook => ("outlook.office365.com", 993, Security::Tls),
            Provider::Yahoo => ("imap.mail.yahoo.com", 993, Security::Tls),
            Provider::Fastmail => ("imap.fastmail.com", 993, Security::Tls),
            Provider::Custom => return None,
        };
        Some(ServerConfig { host: host.into(), port, security })
    }

    pub fn smtp(self) -> Option<ServerConfig> {
        let (host, port, security) = match self {
            Provider::Gmail => ("smtp.gmail.com", 465, Security::Tls),
            Provider::ICloud => ("smtp.mail.me.com", 587, Security::StartTls),
            Provider::Outlook => ("smtp.office365.com", 587, Security::StartTls),
            Provider::Yahoo => ("smtp.mail.yahoo.com", 465, Security::Tls),
            Provider::Fastmail => ("smtp.fastmail.com", 465, Security::Tls),
            Provider::Custom => return None,
        };
        Some(ServerConfig { host: host.into(), port, security })
    }

    /// Short help text shown under the password field.
    pub fn password_help(self) -> &'static str {
        match self {
            Provider::Gmail => "Google requires an App Password: turn on 2-Step Verification, then create one at myaccount.google.com/apppasswords. IMAP must be enabled in Gmail settings.",
            Provider::ICloud => "iCloud requires an app-specific password: create one at account.apple.com > Sign-In and Security > App-Specific Passwords. Use your full @icloud.com address as the username.",
            Provider::Outlook => "Use your account password, or an app password if two-step verification is on.",
            Provider::Yahoo => "Yahoo requires an app password: Account Security > Generate app password.",
            Provider::Fastmail => "Create an app password under Settings > Privacy & Security > App passwords.",
            Provider::Custom => "Enter the IMAP and SMTP details from your mail provider.",
        }
    }

    pub fn color(self) -> &'static str {
        match self {
            Provider::Gmail => "#d93025",
            Provider::ICloud => "#3693f3",
            Provider::Outlook => "#0f6cbd",
            Provider::Yahoo => "#6001d2",
            Provider::Fastmail => "#0067b9",
            Provider::Custom => "#2bb673",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn detects() {
        assert_eq!(Provider::detect("x@icloud.com"), Provider::ICloud);
        assert_eq!(Provider::detect("x@me.com"), Provider::ICloud);
        assert_eq!(Provider::detect("x@gmail.com"), Provider::Gmail);
        assert_eq!(Provider::ICloud.smtp().unwrap().port, 587);
    }
}
