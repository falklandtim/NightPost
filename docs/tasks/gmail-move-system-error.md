# Task: when Gmail turns down a move, try again

**Priority: normal.** Nothing is lost when this happens, but the email bounces back and Tim has to try again. Written 2026-11-18.

Read [AGENTS.md](../../AGENTS.md) first. Where this brief and AGENTS.md disagree, AGENTS.md wins.

## What happened

On 2026-11-18, between 08:50 and 08:52, Tim clicked the trash icon on the email-list rows (the one that shows on hover) in a Gmail inbox. It failed three times. Each time Sync activity and the log said:

```
Moving to Trash failed: MOVE (1 messages, INBOX → [Gmail]/Trash): No Response: System Error (Failure)
```

- The UI took the row out straight away (`app/actions.rs`, `move_thread`) and sent `Cmd::Move` to the worker.
- `engine::move_thread_to` (`crates/nightpost-core/src/engine.rs`) sent `UID MOVE`. Gmail answered `NO System Error (Failure)`, its catch-all for a fault on its own side.
- The local store only changes after the server agrees, so on the next refresh the email came back into the list. No mail was lost or half-moved.
- Nothing else in the log around that time points to a cause. The push connection for the same account was dropping and reconnecting every nine minutes or so ("the server didn't answer in time"), as it had for days. It's not known whether that's related.

## Still unknown: find this out first

- **Were the three failures the same email or different ones?** Ask Tim. If it's always the same email, Gmail probably has a problem with that one message. A retry won't help, and the fix is a clearer message (step 3 below), not step 2.
- **Does it happen again?** Search the log (`~/Library/Logs/NightPost/nightpost.log` on the Mac) for `System Error`. Look only at the error lines, never the mail around them (AGENTS.md rule 3).

Don't build the retry until this shows the failure is short-lived (AGENTS.md rule 6).

## What to do

1. **Write a failing test first** (rule 9). Do it in the style of the existing `retry_on_new_connection` tests in `engine.rs`: a fake session whose first move returns an IMAP `NO` and whose second one succeeds. Don't touch a real mailbox (rule 4).
2. **Retry a move once on a fresh connection** when the server answers `NO` with a server-fault message ("System Error", "[UNAVAILABLE]", "Temporary"). `retry_on_new_connection` won't do this as it is: it only reconnects when the connection has died, and after a `NO` the connection is still alive. Either give it a "should retry" test, or write a small helper next to it. Retry only for these server faults. A `NO` for a missing folder or a bad command must still fail straight away. Use the same logic in `restore_thread` (undo), which sends the same `UID MOVE`. Write a comment saying why it retries (rule 10).
3. **Make the message something Tim can act on** (AGENTS.md section 2). Today the toast quotes the raw IMAP reply. After the retry fails too, say something like "Gmail couldn't move it to Trash just now; it's still in your Inbox. Try again in a minute." Keep the raw reply in Sync activity and the log, since that's where it helps.
4. **Don't fall back to COPY + delete + EXPUNGE** on Gmail to get around a `NO`. If MOVE half-worked on Gmail's side, a fallback could leave two copies, or take the message out of the Inbox without putting it in Trash.
5. **README:** if the message Tim sees changes, update the part of the README that describes Sync activity and errors.

## Done when

- The new test fails without the change and passes with it.
- The full checks in AGENTS.md section 7 pass. Say which machine they ran on.
- Tim has binned some Gmail mail by hand on the installed build, and the log shows any `System Error` being retried. Say plainly that the server-fault path was only tested with the fake session.
