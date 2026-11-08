//! Built-in Model Context Protocol server.
//!
//! `owlpost mcp` speaks MCP (JSON-RPC 2.0, newline-delimited over stdio)
//! so assistants like Claude can search, read, triage, snooze and send mail
//! through the same local database and accounts as the desktop app.
//!
//! Register it with Claude Code:
//! `claude mcp add owlpost -- /path/to/owlpost mcp`

use crate::engine;
use crate::models::*;
use crate::smtp::{self, OutgoingMessage};
use crate::snooze;
use crate::store::Store;
use anyhow::{anyhow, Context, Result};
use serde_json::{json, Value};
use std::io::{BufRead, Write};

pub const PROTOCOL_VERSION: &str = "2025-06-18";

pub struct Server {
    store: Store,
}

/// `"<account_id>:<thread_key>"` – a single opaque id for tools to pass around.
fn thread_id(account_id: AccountId, key: &str) -> String {
    format!("{account_id}:{key}")
}

fn split_thread_id(id: &str) -> Result<(AccountId, String)> {
    let (a, k) = id.split_once(':').ok_or_else(|| anyhow!("thread_id must look like '<account>:<key>', got {id:?}"))?;
    Ok((a.parse().context("thread_id account part")?, k.to_string()))
}

fn tool(name: &str, description: &str, schema: Value, read_only: bool, destructive: bool) -> Value {
    json!({
        "name": name,
        "description": description,
        "inputSchema": schema,
        "annotations": {
            "readOnlyHint": read_only,
            "destructiveHint": destructive,
            "openWorldHint": !read_only,
        }
    })
}

fn tools() -> Value {
    let thread = json!({"type":"string","description":"Thread id from list_threads/search, e.g. \"2:abc@example.com\""});
    json!([
        tool("list_accounts", "List configured mail accounts (iCloud, Gmail, …) with unread counts.",
            json!({"type":"object","properties":{}}), true, false),
        tool("list_folders", "List folders/mailboxes for an account.",
            json!({"type":"object","properties":{"account":{"type":"string","description":"Account email or id. Omit for all accounts."}}}), true, false),
        tool("list_threads", "List conversations in a mailbox, pinned ones first. Snoozed threads are hidden until they wake.",
            json!({"type":"object","properties":{
                "mailbox":{"type":"string","description":"inbox, starred, sent, drafts, archive, spam, trash, snoozed — or a folder id","default":"inbox"},
                "query":{"type":"string","description":"Optional filter. Supports words plus from:, to:, subject:, is:unread, is:starred, is:pinned, has:attachment, category:primary|promotions|social (the inbox tabs)"},
                "limit":{"type":"integer","default":25,"maximum":200}
            }}), true, false),
        tool("search", "Search every account's mail. Same syntax as the app's search bar.",
            json!({"type":"object","required":["query"],"properties":{
                "query":{"type":"string"},
                "limit":{"type":"integer","default":25}
            }}), true, false),
        tool("read_thread", "Read every message in a conversation (headers + plain-text body). Messages with \"html\": true were designed in HTML: if the text looks thin or mentions images, use view_message to see the pictures.",
            json!({"type":"object","required":["thread_id"],"properties":{"thread_id":thread}}), true, false),
        tool("snooze_thread", "Snooze a conversation: hide it until a time, then return it to the inbox as unread.",
            json!({"type":"object","required":["thread_id","until"],"properties":{
                "thread_id":thread,
                "until":{"type":"string","description":"later today, tonight, tomorrow, this weekend, next week, next month, a duration like 3h / 2d, a weekday, or 'YYYY-MM-DD HH:MM' (local time)"}
            }}), false, false),
        tool("unsnooze_thread", "Cancel a snooze and put the conversation back in the inbox now.",
            json!({"type":"object","required":["thread_id"],"properties":{"thread_id":thread}}), false, false),
        tool("list_snoozed", "List snoozed conversations and when each wakes up.",
            json!({"type":"object","properties":{}}), true, false),
        tool("mark_read", "Mark a conversation read or unread (synced to the server).",
            json!({"type":"object","required":["thread_id"],"properties":{"thread_id":thread,"read":{"type":"boolean","default":true}}}), false, false),
        tool("star_thread", "Star or unstar a conversation (IMAP \\Flagged).",
            json!({"type":"object","required":["thread_id"],"properties":{"thread_id":thread,"starred":{"type":"boolean","default":true}}}), false, false),
        tool("sort_sender", "Always put mail from an address in an inbox tab: primary, promotions or social (category \"auto\" removes the rule and goes back to OwlPost's own sorting). Applies to mail already here and future mail, and syncs to every OwlPost.",
            json!({"type":"object","required":["email","category"],"properties":{
                "email":{"type":"string","description":"The sender's address"},
                "category":{"type":"string","enum":["primary","promotions","social","auto"]}}}), false, false),
        tool("pin_thread", "Pin a conversation to the top of the list, or unpin it (pinned=false).",
            json!({"type":"object","required":["thread_id"],"properties":{"thread_id":thread,"pinned":{"type":"boolean","default":true}}}), false, false),
        tool("move_thread", "Move a conversation: archive, trash, spam, inbox, or any folder by name/path (see list_folders). On Gmail, moving to a label adds that label and takes the conversation out of the Inbox.",
            json!({"type":"object","required":["thread_id","to"],"properties":{"thread_id":thread,
                "to":{"type":"string","description":"archive, trash, spam, inbox, or a folder name / path / id"},
                "create":{"type":"boolean","default":false,"description":"Create the folder if it doesn't exist"}}}), false, true),
        tool("label_thread", "Add or remove a label on a conversation. Gmail uses real labels; on other servers a label is a copy in that folder.",
            json!({"type":"object","required":["thread_id","label"],"properties":{"thread_id":thread,
                "label":{"type":"string","description":"Label / folder name, path or id"},
                "remove":{"type":"boolean","default":false},
                "create":{"type":"boolean","default":false,"description":"Create the label if it doesn't exist"}}}), false, false),
        tool("send_email", "Send a new email. This really sends mail from the user's account.",
            json!({"type":"object","required":["to","subject","body"],"properties":{
                "from":{"type":"string","description":"Account email to send from (defaults to the first account)"},
                "to":{"type":"string","description":"Comma separated recipients"},
                "cc":{"type":"string"},"bcc":{"type":"string"},
                "subject":{"type":"string"},"body":{"type":"string","description":"Plain text body"},
                "attachments":{"type":"array","items":{"type":"string"},"description":"Paths of files on this computer to attach"}
            }}), false, true),
        tool("reply", "Reply to the latest message in a conversation. This really sends mail.",
            json!({"type":"object","required":["thread_id","body"],"properties":{
                "thread_id":thread,"body":{"type":"string"},
                "reply_all":{"type":"boolean","default":false},
                "include_quote":{"type":"boolean","default":true}
            }}), false, true),
        tool("view_message", "See one email as designed: its text with an [image N] marker where each picture sits, followed by the pictures themselves (inline, remote and data: images; tracking pixels removed, icons skipped, sized to at most 1200 px). Use it for image-heavy mail such as banner-only marketing, receipts, charts or screenshots pasted into the body.",
            json!({"type":"object","required":["thread_id"],"properties":{
                "thread_id":thread,
                "message_id":{"type":"string","description":"Which message (from read_thread). Defaults to the newest."},
                "images":{"type":"boolean","default":true,"description":"Include the pictures. Remote ones are fetched from the sender's servers, as when the email is viewed."},
                "max_images":{"type":"integer","default":8,"maximum":20}
            }}), true, false),
        tool("get_attachment", "Download an attachment (cached after the first time) and return its file path. Pictures are also returned to look at, and small text files (txt, csv, json, ics…) inline. Open other files, such as PDFs, from the returned path.",
            json!({"type":"object","required":["thread_id","filename"],"properties":{
                "thread_id":thread,
                "filename":{"type":"string","description":"The attachment's name, as listed by read_thread"},
                "message_id":{"type":"string","description":"Which message, when several have an attachment with that name. Defaults to the newest that has it."}
            }}), true, false),
        tool("save_draft", "Save a draft in the account's Drafts folder instead of sending. The user can finish and send it from OwlPost (Edit draft) or any other mail app, including on their phone. Pass thread_id to draft a reply.",
            json!({"type":"object","required":["body"],"properties":{
                "thread_id":{"type":"string","description":"Draft a reply to the latest message in this conversation (recipients, subject and threading are filled in)"},
                "reply_all":{"type":"boolean","default":false},
                "include_quote":{"type":"boolean","default":true},
                "from":{"type":"string","description":"Account email (for a new message; defaults to the first account)"},
                "to":{"type":"string","description":"Comma separated recipients (optional for a draft; replaces a reply's)"},
                "cc":{"type":"string"},"bcc":{"type":"string"},
                "subject":{"type":"string"},
                "body":{"type":"string","description":"Plain text body; the account's signature is added"},
                "attachments":{"type":"array","items":{"type":"string"},"description":"Paths of files on this computer to attach"}
            }}), false, false),
        tool("sync", "Fetch new mail from the IMAP servers now.",
            json!({"type":"object","properties":{"account":{"type":"string","description":"Account email or id; omit for all"}}}), false, false),
    ])
}

impl Server {
    pub fn new(store: Store) -> Self {
        Self { store }
    }

    /// Serve until stdin closes.
    pub fn run_stdio(&mut self) -> Result<()> {
        let stdin = std::io::stdin();
        let mut stdout = std::io::stdout().lock();
        for line in stdin.lock().lines() {
            let line = line?;
            if line.trim().is_empty() {
                continue;
            }
            if let Some(resp) = self.handle_line(&line) {
                writeln!(stdout, "{resp}")?;
                stdout.flush()?;
            }
        }
        Ok(())
    }

    /// Handle one JSON-RPC message; returns the response for requests.
    pub fn handle_line(&mut self, line: &str) -> Option<String> {
        let req: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(json!({"jsonrpc":"2.0","id":null,"error":{"code":-32700,"message":format!("parse error: {e}")}}).to_string())
            }
        };
        if let Some(batch) = req.as_array() {
            let out: Vec<Value> = batch.iter().filter_map(|r| self.handle(r)).collect();
            return (!out.is_empty()).then(|| Value::Array(out).to_string());
        }
        self.handle(&req).map(|v| v.to_string())
    }

    fn handle(&mut self, req: &Value) -> Option<Value> {
        let id = req.get("id").cloned();
        let method = req.get("method").and_then(Value::as_str).unwrap_or("");
        let params = req.get("params").cloned().unwrap_or(json!({}));
        // Notifications get no response.
        let id = id?;
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": params.get("protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL_VERSION),
                "capabilities": {"tools": {"listChanged": false}},
                "serverInfo": {"name": "owlpost", "title": "OwlPost Mail", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "OwlPost gives you the user's email across their iCloud, Gmail and other IMAP accounts. \
                    Use list_threads/search to find conversations, read_thread to read them, and snooze_thread to defer them. \
                    send_email and reply really send mail: confirm the recipients and text with the user before sending."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({"tools": tools()})),
            "resources/list" => Ok(json!({"resources": []})),
            "prompts/list" => Ok(json!({"prompts": []})),
            "tools/call" => {
                let name = params.get("name").and_then(Value::as_str).unwrap_or("");
                let args = params.get("arguments").cloned().unwrap_or(json!({}));
                let _ = crate::snooze_sync::wake_due(&self.store);
                Ok(match self.call_rich(name, &args) {
                    Ok((v, extra)) if extra.is_empty() => json!({"content":[{"type":"text","text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                                    "structuredContent": v, "isError": false}),
                    // Pictures or file text go in `content` after the JSON
                    // (no structuredContent: clients may show only that).
                    Ok((v, extra)) => {
                        let mut content = vec![json!({"type":"text","text": serde_json::to_string_pretty(&v).unwrap_or_default()})];
                        content.extend(extra);
                        json!({"content": content, "isError": false})
                    }
                    Err(e) => json!({"content":[{"type":"text","text": format!("Error: {e:#}")}], "isError": true}),
                })
            }
            _ => Err((-32601, format!("method not found: {method}"))),
        };
        Some(match result {
            Ok(r) => json!({"jsonrpc":"2.0","id":id,"result":r}),
            Err((code, msg)) => json!({"jsonrpc":"2.0","id":id,"error":{"code":code,"message":msg}}),
        })
    }

    fn account_arg(&self, args: &Value) -> Result<Option<Account>> {
        let Some(a) = args.get("account") else { return Ok(None) };
        let s = match a {
            Value::Number(n) => n.to_string(),
            Value::String(s) => s.clone(),
            _ => return Ok(None),
        };
        if let Ok(id) = s.parse::<AccountId>() {
            return Ok(Some(self.store.account(id)?.ok_or_else(|| anyhow!("no account {id}"))?));
        }
        Ok(Some(self.store.account_by_email(&s)?.ok_or_else(|| anyhow!("no account {s}"))?))
    }

    fn account(&self, id: AccountId) -> Result<Account> {
        self.store.account(id)?.ok_or_else(|| anyhow!("no account {id}"))
    }

    fn thread_json(&self, t: &ThreadSummary) -> Result<Value> {
        let acc = self.account(t.account_id)?;
        let snoozed = self.store.snoozed_until(t.account_id, &t.thread_key)?;
        Ok(json!({
            "thread_id": thread_id(t.account_id, &t.thread_key),
            "account": acc.email,
            "subject": t.subject,
            "participants": t.participants.iter().map(Contact::to_header).collect::<Vec<_>>(),
            "snippet": t.snippet,
            "last_message": t.last_date.to_rfc3339(),
            "messages": t.count,
            "unread": t.unread,
            "starred": t.starred,
            "has_attachments": t.has_attachments,
            "pinned": t.pinned,
            "category": crate::category::name(t.category),
            "labels": self.labels_of(t.account_id, &t.thread_key)?,
            "snoozed_until": snoozed.map(|d| d.to_rfc3339()),
        }))
    }

    /// A folder by id, path or name (case-insensitive), optionally created.
    fn find_folder(&self, account: &Account, want: &str, create: bool) -> Result<Folder> {
        let want = want.trim();
        let folders = self.store.folders(account.id)?;
        let hit = folders.iter().find(|f| want.parse::<FolderId>().ok() == Some(f.id)).or_else(|| folders.iter().find(|f| f.path.eq_ignore_ascii_case(want))).or_else(|| folders.iter().find(|f| f.name.eq_ignore_ascii_case(want)));
        if let Some(f) = hit {
            return Ok(f.clone());
        }
        if !create {
            return Err(anyhow!("{} has no folder {want:?} (pass create=true to make it)", account.email));
        }
        let id = engine::create_folder(&self.store, account, want)?;
        self.store.folder(id)?.ok_or_else(|| anyhow!("folder vanished"))
    }

    /// Custom folders / labels a conversation is in.
    fn labels_of(&self, acc: AccountId, key: &str) -> Result<Vec<String>> {
        let ids = self.store.thread_folder_ids(acc, key)?;
        Ok(self.store.folders(acc)?.into_iter().filter(|f| f.role == FolderRole::Other && ids.contains(&f.id)).map(|f| f.path).collect())
    }

    fn view(&self, mailbox: &str) -> Result<MailboxView> {
        if let Ok(id) = mailbox.parse::<FolderId>() {
            return Ok(MailboxView::Folder(id));
        }
        let role = FolderRole::parse(&mailbox.to_ascii_lowercase());
        if role == FolderRole::Other {
            return Err(anyhow!("unknown mailbox {mailbox:?}"));
        }
        Ok(MailboxView::Unified(role))
    }

    /// Run a tool; some also return extra content blocks (pictures, file text).
    pub fn call_rich(&mut self, name: &str, args: &Value) -> Result<(Value, Vec<Value>)> {
        match name {
            "view_message" => self.view_message(args),
            "get_attachment" => self.get_attachment(args),
            _ => Ok((self.call(name, args)?, vec![])),
        }
    }

    /// A message of a thread by Message-ID, or the newest one matching `pick`.
    fn pick_message(&self, args: &Value, pick: impl Fn(&Message) -> bool) -> Result<(Account, Message)> {
        let tid = args.get("thread_id").and_then(Value::as_str).unwrap_or("");
        let (acc, key) = split_thread_id(tid)?;
        let msgs = self.store.thread_messages(acc, &key)?;
        if msgs.is_empty() {
            return Err(anyhow!("thread not found"));
        }
        let want = args.get("message_id").and_then(Value::as_str).map(|s| s.trim().trim_matches(['<', '>'])).filter(|s| !s.is_empty());
        let m = match want {
            Some(id) => msgs.into_iter().find(|m| m.message_id == id).ok_or_else(|| anyhow!("no message {id} in that thread"))?,
            None => msgs.into_iter().rev().find(|m| pick(m)).ok_or_else(|| anyhow!("no matching message in that thread"))?,
        };
        Ok((self.account(acc)?, m))
    }

    fn view_message(&self, args: &Value) -> Result<(Value, Vec<Value>)> {
        let (account, m) = self.pick_message(args, |_| true)?;
        let images = args.get("images").and_then(Value::as_bool).unwrap_or(true);
        let max = args.get("max_images").and_then(Value::as_u64).unwrap_or(8).min(20) as usize;
        let opts = crate::email_view::Options { remote: images, max_images: if images { max } else { 0 } };
        let v = crate::email_view::view(&self.store, &account, &m, &opts)?;
        let mut extra = vec![];
        let mut list = vec![];
        for img in &v.images {
            let mut e = json!({"image": img.n, "alt": img.alt});
            match (&img.picture, &img.skipped) {
                (Some(p), _) => {
                    use base64::Engine;
                    e["included"] = json!(true);
                    e["size"] = json!(format!("{}x{}", p.width, p.height));
                    extra.push(json!({"type":"text","text": format!("[image {}{}]", img.n, if img.alt.is_empty() { String::new() } else { format!(": {}", img.alt) })}));
                    extra.push(json!({"type":"image","mimeType": p.mime, "data": base64::engine::general_purpose::STANDARD.encode(&p.data)}));
                }
                (None, why) => {
                    e["included"] = json!(false);
                    e["reason"] = json!(why.clone().unwrap_or_else(|| "not loaded".into()));
                }
            }
            list.push(e);
        }
        let out = json!({
            "message_id": m.message_id,
            "from": m.from.to_header(),
            "date": m.date.to_rfc3339(),
            "subject": m.subject,
            "html": v.html,
            "trackers_blocked": v.trackers,
            "attachments": m.attachments.iter().map(|a| a.filename.clone()).collect::<Vec<_>>(),
            "images": list,
            "text": v.text,
        });
        Ok((out, extra))
    }

    fn get_attachment(&self, args: &Value) -> Result<(Value, Vec<Value>)> {
        let name = args.get("filename").and_then(Value::as_str).unwrap_or("").trim().to_string();
        if name.is_empty() {
            return Err(anyhow!("filename is required"));
        }
        let (account, m) = self.pick_message(args, |m| m.attachments.iter().any(|a| a.filename.eq_ignore_ascii_case(&name)))?;
        let index = m
            .attachments
            .iter()
            .position(|a| a.filename.eq_ignore_ascii_case(&name))
            .ok_or_else(|| anyhow!("that message has no attachment called {name:?}"))?;
        let meta = m.attachments[index].clone();
        let path = engine::attachment_file(&self.store, &account, &m, index)?;
        let mut out = json!({"filename": meta.filename, "type": meta.mime_type, "size": meta.size, "path": path.display().to_string(),
                             "message_id": m.message_id});
        let mut extra = vec![];
        let mime = meta.mime_type.to_ascii_lowercase();
        let ext = meta.filename.rsplit('.').next().unwrap_or("").to_ascii_lowercase();
        let texty = mime.starts_with("text/") || ["json", "csv", "tsv", "txt", "md", "ics", "vcf", "xml", "eml", "log"].contains(&ext.as_str());
        if mime.starts_with("image/") || ["jpg", "jpeg", "png", "gif", "webp", "heic"].contains(&ext.as_str()) {
            match std::fs::read(&path).map_err(anyhow::Error::from).and_then(|d| crate::email_view::picture(&d)) {
                Ok(p) => {
                    use base64::Engine;
                    out["shown"] = json!(format!("picture, {}x{}", p.width, p.height));
                    extra.push(json!({"type":"image","mimeType": p.mime, "data": base64::engine::general_purpose::STANDARD.encode(&p.data)}));
                }
                Err(e) => out["note"] = json!(format!("couldn't show the picture ({e:#}); open it from path")),
            }
        } else if texty && meta.size <= 200_000 {
            let data = std::fs::read(&path)?;
            let text = String::from_utf8_lossy(&data).to_string();
            out["shown"] = json!("text");
            extra.push(json!({"type":"text","text": text}));
        } else {
            out["note"] = json!("Read the file at path to see it (PDFs, documents and spreadsheets can be opened from there).");
        }
        Ok((out, extra))
    }

    pub fn call(&mut self, name: &str, args: &Value) -> Result<Value> {
        let s = |k: &str| args.get(k).and_then(Value::as_str).unwrap_or("").to_string();
        let limit = args.get("limit").and_then(Value::as_u64).unwrap_or(25).min(200) as usize;
        match name {
            "list_accounts" => {
                let mut out = vec![];
                for a in self.store.accounts()? {
                    let inbox_unread = self
                        .store
                        .folder_by_role(a.id, FolderRole::Inbox)?
                        .map(|f| f.unread)
                        .unwrap_or(0);
                    out.push(json!({"id":a.id,"email":a.email,"name":a.display_name,
                        "imap":format!("{}:{}", a.imap.host, a.imap.port),"inbox_unread":inbox_unread,"demo":a.demo}));
                }
                Ok(json!({"accounts": out}))
            }
            "list_folders" => {
                let accounts = match self.account_arg(args)? {
                    Some(a) => vec![a],
                    None => self.store.accounts()?,
                };
                let mut out = vec![];
                for a in accounts {
                    for f in self.store.folders(a.id)?.into_iter().filter(|f| !crate::snooze_sync::is_internal_folder(&f.path)) {
                        out.push(json!({"id":f.id,"account":a.email,"path":f.path,"role":f.role.as_str(),"unread":f.unread}));
                    }
                }
                Ok(json!({"folders": out}))
            }
            "list_threads" | "search" => {
                let mailbox = if name == "search" { "all".to_string() } else { s("mailbox") };
                let mailbox = if mailbox.is_empty() { "inbox".to_string() } else { mailbox };
                if mailbox == "snoozed" {
                    return self.call("list_snoozed", args);
                }
                let query = s("query");
                let view = if mailbox == "all" { MailboxView::Everything } else { self.view(&mailbox)? };
                let threads = self.store.threads(&view, &query, limit)?;
                let list: Vec<Value> = threads.iter().map(|t| self.thread_json(t)).collect::<Result<_>>()?;
                Ok(json!({"mailbox": mailbox, "count": list.len(), "threads": list}))
            }
            "read_thread" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                let msgs = self.store.thread_messages(acc, &key)?;
                if msgs.is_empty() {
                    return Err(anyhow!("thread not found"));
                }
                let out: Vec<Value> = msgs
                    .iter()
                    .map(|m| {
                        json!({
                            "message_id": m.message_id,
                            "date": m.date.to_rfc3339(),
                            "from": m.from.to_header(),
                            "to": Contact::join(&m.to),
                            "cc": Contact::join(&m.cc),
                            "subject": m.subject,
                            "unread": m.unread,
                            "html": m.body_html.is_some(),
                            "attachments": m.attachments.iter().map(|a| json!({"filename": a.filename, "type": a.mime_type, "size": a.size})).collect::<Vec<_>>(),
                            "body": m.body_text,
                        })
                    })
                    .collect();
                Ok(json!({"thread_id": thread_id(acc, &key), "subject": msgs[0].subject,
                          "snoozed_until": self.store.snoozed_until(acc, &key)?.map(|d| d.to_rfc3339()),
                          "messages": out}))
            }
            "snooze_thread" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                if self.store.thread_messages(acc, &key)?.is_empty() {
                    return Err(anyhow!("thread not found"));
                }
                let until = snooze::parse_when(&s("until"), chrono::Local::now())?;
                if until <= chrono::Utc::now() {
                    return Err(anyhow!("snooze time must be in the future"));
                }
                let account = self.store.account(acc)?.ok_or_else(|| anyhow!("account not found"))?;
                crate::snooze_sync::snooze_thread(&self.store, &account, &key, until)?;
                Ok(json!({"thread_id": thread_id(acc, &key), "snoozed_until": until.to_rfc3339(), "label": snooze::describe(until)}))
            }
            "unsnooze_thread" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                let account = self.store.account(acc)?.ok_or_else(|| anyhow!("account not found"))?;
                crate::snooze_sync::unsnooze_thread(&self.store, &account, &key)?;
                Ok(json!({"thread_id": thread_id(acc, &key), "snoozed": false}))
            }
            "list_snoozed" => {
                let list: Vec<Value> = self
                    .store
                    .snoozed_threads()?
                    .iter()
                    .map(|(t, until)| {
                        let mut v = self.thread_json(t)?;
                        v["wakes"] = json!(snooze::describe(*until));
                        Ok(v)
                    })
                    .collect::<Result<_>>()?;
                Ok(json!({"count": list.len(), "threads": list}))
            }
            "mark_read" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                let read = args.get("read").and_then(Value::as_bool).unwrap_or(true);
                engine::set_thread_unread(&self.store, &self.account(acc)?, &key, !read)?;
                Ok(json!({"thread_id": thread_id(acc, &key), "read": read}))
            }
            "star_thread" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                let starred = args.get("starred").and_then(Value::as_bool).unwrap_or(true);
                engine::set_thread_starred(&self.store, &self.account(acc)?, &key, starred)?;
                Ok(json!({"thread_id": thread_id(acc, &key), "starred": starred}))
            }
            "sort_sender" => {
                let email = s("email").trim().to_lowercase();
                if !email.contains('@') {
                    return Err(anyhow!("{email:?} isn't an email address"));
                }
                let cat = s("category");
                let category = match cat.trim().to_ascii_lowercase().as_str() {
                    "auto" => None,
                    c => Some(crate::category::parse(c).ok_or_else(|| anyhow!("unknown category {cat:?}: use primary, promotions, social or auto"))?),
                };
                crate::snooze_sync::set_sender_rule(&self.store, &email, category)?;
                Ok(json!({"email": email, "category": category.map(crate::category::name).unwrap_or("auto")}))
            }
            "pin_thread" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                if self.store.thread_messages(acc, &key)?.is_empty() {
                    return Err(anyhow!("thread not found"));
                }
                let pinned = args.get("pinned").and_then(Value::as_bool).unwrap_or(true);
                if pinned {
                    self.store.pin(acc, &key)?;
                } else {
                    self.store.unpin(acc, &key)?;
                }
                Ok(json!({"thread_id": thread_id(acc, &key), "pinned": pinned}))
            }
            "move_thread" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                let account = self.account(acc)?;
                if self.store.thread_messages(acc, &key)?.is_empty() {
                    return Err(anyhow!("thread not found"));
                }
                let to = s("to");
                let role = FolderRole::parse(&to.to_ascii_lowercase());
                let target = if matches!(role, FolderRole::Archive | FolderRole::Trash | FolderRole::Spam | FolderRole::Inbox) {
                    engine::folder_for_role(&self.store, &account, role)?
                } else {
                    self.find_folder(&account, &to, args.get("create").and_then(Value::as_bool).unwrap_or(false))?
                };
                engine::move_thread_to(&self.store, &account, &key, &target, None)?;
                Ok(json!({"thread_id": thread_id(acc, &key), "moved_to": target.path}))
            }
            "label_thread" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                let account = self.account(acc)?;
                if self.store.thread_messages(acc, &key)?.is_empty() {
                    return Err(anyhow!("thread not found"));
                }
                let label = self.find_folder(&account, &s("label"), args.get("create").and_then(Value::as_bool).unwrap_or(false))?;
                let remove = args.get("remove").and_then(Value::as_bool).unwrap_or(false);
                engine::set_label(&self.store, &account, &key, &label, !remove)?;
                Ok(json!({"thread_id": thread_id(acc, &key), "label": label.path, "applied": !remove,
                          "labels": self.labels_of(acc, &key)?}))
            }
            "send_email" => {
                let from = s("from");
                let account = if from.is_empty() {
                    self.store.accounts()?.into_iter().next().ok_or_else(|| anyhow!("no accounts configured"))?
                } else {
                    self.store.account_by_email(&from)?.ok_or_else(|| anyhow!("no account {from}"))?
                };
                let out = OutgoingMessage {
                    to: Contact::parse_list(&s("to")),
                    cc: Contact::parse_list(&s("cc")),
                    bcc: Contact::parse_list(&s("bcc")),
                    subject: s("subject"),
                    ..Default::default()
                };
                let (body, html_signature) = with_signature(&self.store, &s("body"), &account);
                let attachments = attachment_paths(args)?
                    .iter()
                    .map(|p| smtp::OutgoingAttachment::from_path(std::path::Path::new(p)))
                    .collect::<Result<Vec<_>>>()?;
                let out = OutgoingMessage { body, html_signature, attachments, ..out };
                if smtp::encoded_size(&out) > smtp::SIZE_LIMIT {
                    return Err(anyhow!("attachments are too large to email (limit about 25 MB)"));
                }
                let id = engine::send(&self.store, &account, &out)?;
                let names: Vec<_> = out.attachments.iter().map(|a| a.filename.clone()).collect();
                Ok(json!({"sent": true, "from": account.email, "message_id": id, "attachments": names}))
            }
            "save_draft" => {
                let tid = s("thread_id");
                let (account, draft) = if tid.trim().is_empty() {
                    let from = s("from");
                    let account = if from.is_empty() {
                        self.store.accounts()?.into_iter().next().ok_or_else(|| anyhow!("no accounts configured"))?
                    } else {
                        self.store.account_by_email(&from)?.ok_or_else(|| anyhow!("no account {from}"))?
                    };
                    (account, OutgoingMessage::default())
                } else {
                    let (acc, key) = split_thread_id(&tid)?;
                    let account = self.account(acc)?;
                    let msgs = self.store.thread_messages(acc, &key)?;
                    let last = msgs.last().ok_or_else(|| anyhow!("thread not found"))?;
                    let mut d = reply_draft(&account, last, args.get("reply_all").and_then(Value::as_bool).unwrap_or(false));
                    if args.get("include_quote").and_then(Value::as_bool).unwrap_or(true) {
                        d.body = smtp::quote(last);
                    }
                    (account, d)
                };
                let pick = |k: &str, default: Vec<Contact>| if s(k).trim().is_empty() { default } else { Contact::parse_list(&s(k)) };
                let (body, html_signature) = with_signature(&self.store, &s("body"), &account);
                let attachments = attachment_paths(args)?
                    .iter()
                    .map(|p| smtp::OutgoingAttachment::from_path(std::path::Path::new(p)))
                    .collect::<Result<Vec<_>>>()?;
                let out = OutgoingMessage {
                    to: pick("to", draft.to),
                    cc: pick("cc", draft.cc),
                    bcc: pick("bcc", vec![]),
                    subject: if s("subject").trim().is_empty() { draft.subject } else { s("subject") },
                    body: format!("{body}{}", draft.body),
                    html_signature,
                    attachments,
                    in_reply_to: draft.in_reply_to,
                    references: draft.references,
                    html: None,
                };
                if smtp::encoded_size(&out) > smtp::SIZE_LIMIT {
                    return Err(anyhow!("attachments are too large to email (limit about 25 MB)"));
                }
                let id = engine::save_draft(&self.store, &account, &out, None)?;
                let drafts = engine::folder_for_role(&self.store, &account, FolderRole::Drafts)?;
                let thread = self.store.copy_in_role(account.id, &id, FolderRole::Drafts)?.map(|m| thread_id(account.id, &m.thread_key));
                Ok(json!({"saved": true, "account": account.email, "folder": drafts.path, "message_id": id,
                          "thread_id": thread, "to": Contact::join(&out.to), "subject": out.subject}))
            }
            "reply" => {
                let (acc, key) = split_thread_id(&s("thread_id"))?;
                let account = self.account(acc)?;
                let msgs = self.store.thread_messages(acc, &key)?;
                let last = msgs.last().ok_or_else(|| anyhow!("thread not found"))?;
                let reply_all = args.get("reply_all").and_then(Value::as_bool).unwrap_or(false);
                let quote = args.get("include_quote").and_then(Value::as_bool).unwrap_or(true);
                let draft = reply_draft(&account, last, reply_all);
                let (mut body, html_signature) = with_signature(&self.store, &s("body"), &account);
                if quote {
                    body.push_str(&smtp::quote(last));
                }
                let out = OutgoingMessage { body, html_signature, ..draft };
                let id = engine::send(&self.store, &account, &out)?;
                Ok(json!({"sent": true, "to": Contact::join(&out.to), "cc": Contact::join(&out.cc), "message_id": id}))
            }
            "sync" => {
                let accounts = match self.account_arg(args)? {
                    Some(a) => vec![a],
                    None => self.store.accounts()?,
                };
                let mut out = vec![];
                for a in accounts {
                    let r = engine::sync_account(&self.store, &a, |_| {});
                    out.push(match r {
                        Ok(n) => json!({"account": a.email, "new_messages": n}),
                        Err(e) => json!({"account": a.email, "error": format!("{e:#}")}),
                    });
                }
                Ok(json!({"results": out}))
            }
            _ => Err(anyhow!("unknown tool {name}")),
        }
    }
}

/// Append the account's default signature; HTML ones also set the HTML part.
fn with_signature(store: &Store, body: &str, account: &Account) -> (String, Option<smtp::HtmlSignature>) {
    match store.default_signature(account.id).ok().flatten() {
        Some(sig) if !sig.text().trim().is_empty() => {
            let text = sig.text();
            let html = sig.html.then(|| smtp::HtmlSignature { text: text.clone(), html: sig.body.clone() });
            (format!("{body}\n\n{text}"), html)
        }
        _ => (body.to_string(), None),
    }
}

/// Build To/Cc/subject/threading headers for a reply to `m`.
/// The `attachments` argument: a list of paths (a single string is accepted too).
fn attachment_paths(args: &Value) -> Result<Vec<String>> {
    match args.get("attachments") {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::String(p)) => Ok(vec![p.clone()]),
        Some(Value::Array(a)) => a.iter().map(|v| v.as_str().map(str::to_string).ok_or_else(|| anyhow!("attachments must be file paths"))).collect(),
        Some(_) => Err(anyhow!("attachments must be a list of file paths")),
    }
}

pub fn reply_draft(account: &Account, m: &Message, reply_all: bool) -> OutgoingMessage {
    let me = account.email.to_ascii_lowercase();
    let from_me = m.from.email.eq_ignore_ascii_case(&me);
    // Replying to our own message goes back to its recipients.
    let mut to = if from_me { m.to.clone() } else { vec![m.from.clone()] };
    let mut cc = vec![];
    if reply_all {
        for c in m.to.iter().chain(&m.cc) {
            let e = c.email.to_ascii_lowercase();
            if e != me && !to.iter().any(|t| t.email.eq_ignore_ascii_case(&e)) && !cc.iter().any(|t: &Contact| t.email.eq_ignore_ascii_case(&e)) {
                cc.push(c.clone());
            }
        }
    }
    to.retain(|c| !c.email.eq_ignore_ascii_case(&me) || from_me);
    let subject = if m.subject.to_ascii_lowercase().starts_with("re:") { m.subject.clone() } else { format!("Re: {}", m.subject) };
    let mut references = m.references.clone();
    references.push(m.message_id.clone());
    OutgoingMessage {
        to,
        cc,
        bcc: vec![],
        subject,
        body: String::new(),
        in_reply_to: Some(m.message_id.clone()),
        references,
        html_signature: None,
        attachments: vec![],
        html: None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn server() -> Server {
        let p = crate::test_db_path("sp-mcp");
        let store = Store::open(&p).unwrap();
        crate::demo::seed(&store).unwrap();
        Server::new(store)
    }

    fn rpc(s: &mut Server, id: i64, method: &str, params: Value) -> Value {
        let line = json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}).to_string();
        serde_json::from_str(&s.handle_line(&line).unwrap()).unwrap()
    }

    #[test]
    fn handshake_and_tools() {
        let mut s = server();
        let r = rpc(&mut s, 1, "initialize", json!({"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"t","version":"1"}}));
        assert_eq!(r["result"]["serverInfo"]["name"], "owlpost");
        assert!(s.handle_line(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#).is_none());
        let r = rpc(&mut s, 2, "tools/list", json!({}));
        let names: Vec<&str> = r["result"]["tools"].as_array().unwrap().iter().map(|t| t["name"].as_str().unwrap()).collect();
        assert!(names.contains(&"snooze_thread") && names.contains(&"send_email"));
    }

    #[test]
    fn categories_via_tools() {
        let mut s = server();
        let list = |s: &mut Server, id: i64, q: &str| -> Vec<String> {
            let r = rpc(s, id, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox","query":q}}));
            r["result"]["structuredContent"]["threads"].as_array().unwrap().iter().map(|t| t["subject"].as_str().unwrap().to_string()).collect()
        };
        let promos = list(&mut s, 1, "category:promotions");
        assert!(promos.len() == 1 && promos[0].contains("November roast"), "{promos:?}");
        let all = list(&mut s, 2, "").len();
        let primary = list(&mut s, 3, "category:primary").len();
        let social = list(&mut s, 4, "category:social").len();
        assert_eq!(primary + 1 + social, all, "every conversation is in exactly one tab");
        // Move Maya's mail to Social, then back to automatic.
        let r = rpc(&mut s, 5, "tools/call", json!({"name":"sort_sender","arguments":{"email":"Maya@Chen.family","category":"social"}}));
        assert_eq!(r["result"]["structuredContent"]["category"], "social");
        assert!(list(&mut s, 6, "category:social").iter().any(|t| t.contains("Dinner")));
        rpc(&mut s, 7, "tools/call", json!({"name":"sort_sender","arguments":{"email":"maya@chen.family","category":"auto"}}));
        assert!(!list(&mut s, 8, "category:social").iter().any(|t| t.contains("Dinner")));
        let r = rpc(&mut s, 9, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox","query":"category:promotions"}}));
        assert_eq!(r["result"]["structuredContent"]["threads"][0]["category"], "promotions");
    }

    #[test]
    fn snooze_via_tools() {
        let mut s = server();
        let r = rpc(&mut s, 1, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox"}}));
        let threads = r["result"]["structuredContent"]["threads"].as_array().unwrap().clone();
        let tid = threads[0]["thread_id"].as_str().unwrap().to_string();
        let r = rpc(&mut s, 2, "tools/call", json!({"name":"snooze_thread","arguments":{"thread_id":tid,"until":"tomorrow"}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        let r = rpc(&mut s, 3, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox"}}));
        assert_eq!(r["result"]["structuredContent"]["threads"].as_array().unwrap().len(), threads.len() - 1);
        let r = rpc(&mut s, 4, "tools/call", json!({"name":"list_snoozed","arguments":{}}));
        assert_eq!(r["result"]["structuredContent"]["count"], 1);
        let r = rpc(&mut s, 5, "tools/call", json!({"name":"read_thread","arguments":{"thread_id":tid}}));
        assert!(r["result"]["structuredContent"]["snoozed_until"].is_string());
    }

    #[test]
    fn pin_via_tools() {
        let mut s = server();
        let r = rpc(&mut s, 1, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox"}}));
        let threads = r["result"]["structuredContent"]["threads"].as_array().unwrap().clone();
        let tid = threads.last().unwrap()["thread_id"].as_str().unwrap().to_string();
        let r = rpc(&mut s, 2, "tools/call", json!({"name":"pin_thread","arguments":{"thread_id":tid}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        let r = rpc(&mut s, 3, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox"}}));
        let first = &r["result"]["structuredContent"]["threads"][0];
        assert_eq!(first["thread_id"], tid.as_str());
        assert_eq!(first["pinned"], true);
    }

    #[test]
    fn move_and_label_via_tools() {
        let mut s = server();
        let r = rpc(&mut s, 1, "tools/call", json!({"name":"search","arguments":{"query":"subject:roadmap"}}));
        let tid = r["result"]["structuredContent"]["threads"][0]["thread_id"].as_str().unwrap().to_string();
        let r = rpc(&mut s, 2, "tools/call", json!({"name":"label_thread","arguments":{"thread_id":tid,"label":"Clients","create":true}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert_eq!(r["result"]["structuredContent"]["labels"], json!(["Clients"]));
        // Labelling leaves it in the inbox.
        let r = rpc(&mut s, 3, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox","query":"subject:roadmap"}}));
        assert_eq!(r["result"]["structuredContent"]["count"], 1);
        let r = rpc(&mut s, 4, "tools/call", json!({"name":"label_thread","arguments":{"thread_id":tid,"label":"clients","remove":true}}));
        assert_eq!(r["result"]["structuredContent"]["labels"], json!([]));
        // Moving to a folder takes it out of the inbox.
        let r = rpc(&mut s, 5, "tools/call", json!({"name":"move_thread","arguments":{"thread_id":tid,"to":"Projects/OwlPost"}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        let r = rpc(&mut s, 6, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"inbox","query":"subject:roadmap"}}));
        assert_eq!(r["result"]["structuredContent"]["count"], 0);
        let r = rpc(&mut s, 7, "tools/call", json!({"name":"move_thread","arguments":{"thread_id":tid,"to":"Nope"}}));
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn demo_reply_lands_in_thread() {
        let mut s = server();
        let r = rpc(&mut s, 1, "tools/call", json!({"name":"search","arguments":{"query":"subject:Dinner"}}));
        let t = &r["result"]["structuredContent"]["threads"][0];
        let tid = t["thread_id"].as_str().unwrap().to_string();
        let before = t["messages"].as_i64().unwrap();
        let r = rpc(&mut s, 2, "tools/call", json!({"name":"reply","arguments":{"thread_id":tid,"body":"Perfect, see you!"}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert!(r["result"]["structuredContent"]["to"].as_str().unwrap().contains("maya@chen.family"));
        let r = rpc(&mut s, 3, "tools/call", json!({"name":"read_thread","arguments":{"thread_id":tid}}));
        assert_eq!(r["result"]["structuredContent"]["messages"].as_array().unwrap().len() as i64, before + 1);
    }

    #[test]
    fn drafts_view_and_attachments_via_tools() {
        let mut s = server();
        let r = rpc(&mut s, 1, "tools/call", json!({"name":"search","arguments":{"query":"subject:Dinner"}}));
        let tid = r["result"]["structuredContent"]["threads"][0]["thread_id"].as_str().unwrap().to_string();
        let sent = |s: &mut Server| rpc(s, 0, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"sent","limit":200}}))["result"]["structuredContent"]["count"].clone();
        let sent_before = sent(&mut s);
        // A reply draft lands in Drafts, in the same conversation, and sends nothing.
        let r = rpc(&mut s, 2, "tools/call", json!({"name":"save_draft","arguments":{"thread_id":tid,"body":"Sounds great","bcc":"me@example.com"}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        let d = &r["result"]["structuredContent"];
        assert_eq!(d["thread_id"], tid.as_str());
        assert!(d["to"].as_str().unwrap().contains("maya@chen.family"));
        let r = rpc(&mut s, 3, "tools/call", json!({"name":"list_threads","arguments":{"mailbox":"drafts"}}));
        assert!(r["result"]["structuredContent"]["threads"].as_array().unwrap().iter().any(|t| t["thread_id"] == tid.as_str()));
        assert_eq!(sent(&mut s), sent_before);
        // A new draft needs no recipients.
        let r = rpc(&mut s, 5, "tools/call", json!({"name":"save_draft","arguments":{"subject":"Ideas","body":"…"}}));
        assert_eq!(r["result"]["isError"], false, "{r}");

        // view_message gives the text (and no structuredContent when it adds pictures).
        let r = rpc(&mut s, 6, "tools/call", json!({"name":"view_message","arguments":{"thread_id":tid,"images":false}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        assert!(r["result"]["structuredContent"]["text"].is_string(), "{r}");

        // get_attachment returns a path to open.
        let r = rpc(&mut s, 7, "tools/call", json!({"name":"search","arguments":{"query":"has:attachment"}}));
        let t = &r["result"]["structuredContent"]["threads"][0];
        let tid = t["thread_id"].as_str().unwrap().to_string();
        let r = rpc(&mut s, 8, "tools/call", json!({"name":"read_thread","arguments":{"thread_id":tid}}));
        let name = r["result"]["structuredContent"]["messages"].as_array().unwrap().iter()
            .find_map(|m| m["attachments"].as_array().and_then(|a| a.first()).map(|a| a["filename"].as_str().unwrap().to_string())).unwrap();
        std::env::set_var("OWLPOST_CACHE", std::env::temp_dir().join(format!("sp-mcp-cache-{}", std::process::id())));
        let r = rpc(&mut s, 9, "tools/call", json!({"name":"get_attachment","arguments":{"thread_id":tid,"filename":name}}));
        assert_eq!(r["result"]["isError"], false, "{r}");
        let body = &r["result"]["content"][0]["text"];
        let v: Value = serde_json::from_str(body.as_str().unwrap()).unwrap();
        assert!(std::path::Path::new(v["path"].as_str().unwrap()).is_file(), "{v}");
        let r = rpc(&mut s, 10, "tools/call", json!({"name":"get_attachment","arguments":{"thread_id":tid,"filename":"nope.pdf"}}));
        assert_eq!(r["result"]["isError"], true);
    }

    #[test]
    fn errors_are_tool_errors() {
        let mut s = server();
        let r = rpc(&mut s, 1, "tools/call", json!({"name":"snooze_thread","arguments":{"thread_id":"1:nope","until":"whenever"}}));
        assert_eq!(r["result"]["isError"], true);
        let r = rpc(&mut s, 2, "bogus/method", json!({}));
        assert_eq!(r["error"]["code"], -32601);
    }
}
