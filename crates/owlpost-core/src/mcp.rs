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
                "query":{"type":"string","description":"Optional filter. Supports words plus from:, to:, subject:, is:unread, is:starred, is:pinned, has:attachment"},
                "limit":{"type":"integer","default":25,"maximum":200}
            }}), true, false),
        tool("search", "Search every account's mail. Same syntax as the app's search bar.",
            json!({"type":"object","required":["query"],"properties":{
                "query":{"type":"string"},
                "limit":{"type":"integer","default":25}
            }}), true, false),
        tool("read_thread", "Read every message in a conversation (headers + plain-text body).",
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
                Ok(match self.call(name, &args) {
                    Ok(v) => json!({"content":[{"type":"text","text": serde_json::to_string_pretty(&v).unwrap_or_default()}],
                                    "structuredContent": v, "isError": false}),
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
                            "attachments": m.attachments.iter().map(|a| a.filename.clone()).collect::<Vec<_>>(),
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
    fn errors_are_tool_errors() {
        let mut s = server();
        let r = rpc(&mut s, 1, "tools/call", json!({"name":"snooze_thread","arguments":{"thread_id":"1:nope","until":"whenever"}}));
        assert_eq!(r["result"]["isError"], true);
        let r = rpc(&mut s, 2, "bogus/method", json!({}));
        assert_eq!(r["error"]["code"], -32601);
    }
}
