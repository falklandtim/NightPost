//! The system address book: the Mac's Contacts app. Only inside
//! OwlPost.app, whose Info.plist explains why it asks (macOS ends any app
//! that asks without). Elsewhere, and in tests, it's empty.

use owlpost_core::Contact;

/// Address-book people whose name or email contains `query`, best first.
/// Never blocks: the first call starts loading in the background and
/// returns nothing until the list is ready.
pub fn search(query: &str, limit: usize) -> Vec<Contact> {
    let q = query.trim().to_lowercase();
    if q.is_empty() {
        return Vec::new();
    }
    let Some(all) = imp::people() else { return Vec::new() };
    let mut hits: Vec<(u8, &Contact)> = all
        .iter()
        .filter_map(|c| {
            let name = c.name.to_lowercase();
            let email = c.email.to_lowercase();
            let rank = if name.starts_with(&q) || email.starts_with(&q) {
                0
            } else if name.split_whitespace().any(|w| w.starts_with(&q)) {
                1
            } else if name.contains(&q) || email.contains(&q) {
                2
            } else {
                return None;
            };
            Some((rank, c))
        })
        .collect();
    hits.sort_by_key(|(r, c)| (*r, c.name.to_lowercase()));
    hits.into_iter().take(limit).map(|(_, c)| c.clone()).collect()
}

pub use imp::photo;

#[cfg(all(target_os = "macos", not(test)))]
mod imp {
    use objc2::rc::Retained;
    use objc2::runtime::{Bool, ProtocolObject};
    use objc2::AllocAnyThread;
    use objc2_contacts::{
        CNAuthorizationStatus, CNContact, CNContactEmailAddressesKey, CNContactFamilyNameKey, CNContactFetchRequest,
        CNContactGivenNameKey, CNContactOrganizationNameKey, CNContactStore, CNContactThumbnailImageDataKey, CNEntityType,
        CNKeyDescriptor,
    };
    use objc2_foundation::{ns_string, NSArray, NSBundle, NSError, NSString};
    use owlpost_core::Contact;
    use std::ptr::NonNull;
    use std::sync::{Arc, Mutex, OnceLock};
    use std::time::{Duration, Instant};

    fn allowed() -> bool {
        static ALLOWED: OnceLock<bool> = OnceLock::new();
        *ALLOWED.get_or_init(|| unsafe {
            if NSBundle::mainBundle().objectForInfoDictionaryKey(ns_string!("NSContactsUsageDescription")).is_none() {
                return false;
            }
            let status = CNContactStore::authorizationStatusForEntityType(CNEntityType::Contacts);
            if status == CNAuthorizationStatus::NotDetermined {
                // Ask once; this thread waits for the answer.
                let (tx, rx) = std::sync::mpsc::channel();
                let block = block2::RcBlock::new(move |granted: Bool, _e: *mut NSError| {
                    let _ = tx.send(granted.as_bool());
                });
                CNContactStore::new().requestAccessForEntityType_completionHandler(CNEntityType::Contacts, &block);
                return rx.recv_timeout(Duration::from_secs(120)).unwrap_or(false);
            }
            status == CNAuthorizationStatus::Authorized || status == CNAuthorizationStatus::Limited
        })
    }

    pub fn photo(email: &str) -> Option<Vec<u8>> {
        if !allowed() {
            return None;
        }
        unsafe {
            let store = CNContactStore::new();
            let predicate = CNContact::predicateForContactsMatchingEmailAddress(&NSString::from_str(email));
            let key: &ProtocolObject<dyn CNKeyDescriptor> = ProtocolObject::from_ref(CNContactThumbnailImageDataKey);
            let keys = NSArray::from_slice(&[key]);
            let found = store.unifiedContactsMatchingPredicate_keysToFetch_error(&predicate, &keys).ok()?;
            found.iter().find_map(|c| c.thumbnailImageData()).map(|d| d.to_vec())
        }
    }

    /// Every (name, email) pair in Contacts.
    fn load() -> Vec<Contact> {
        if !allowed() {
            return Vec::new();
        }
        let out = std::cell::RefCell::new(Vec::new());
        unsafe {
            let keys: [&ProtocolObject<dyn CNKeyDescriptor>; 4] = [
                ProtocolObject::from_ref(CNContactGivenNameKey),
                ProtocolObject::from_ref(CNContactFamilyNameKey),
                ProtocolObject::from_ref(CNContactOrganizationNameKey),
                ProtocolObject::from_ref(CNContactEmailAddressesKey),
            ];
            let keys = NSArray::from_slice(&keys);
            let request: Retained<CNContactFetchRequest> = CNContactFetchRequest::initWithKeysToFetch(CNContactFetchRequest::alloc(), &keys);
            let block = block2::RcBlock::new(|c: NonNull<CNContact>, _stop: NonNull<Bool>| {
                let c = c.as_ref();
                let name = format!("{} {}", c.givenName(), c.familyName()).trim().to_string();
                let name = if name.is_empty() { c.organizationName().to_string() } else { name };
                for e in c.emailAddresses().iter() {
                    let email = e.value().to_string();
                    if email.contains('@') {
                        out.borrow_mut().push(Contact { name: name.clone(), email: email.trim().to_string() });
                    }
                }
            });
            let _ = CNContactStore::new().enumerateContactsWithFetchRequest_error_usingBlock(&request, None, &block);
        }
        out.into_inner()
    }

    struct Cache {
        list: Option<Arc<Vec<Contact>>>,
        loaded: Option<Instant>,
        loading: bool,
    }

    /// The cached list, refreshed in the background every few minutes so
    /// edits in Contacts show up without a restart.
    pub fn people() -> Option<Arc<Vec<Contact>>> {
        static CACHE: Mutex<Cache> = Mutex::new(Cache { list: None, loaded: None, loading: false });
        let mut c = CACHE.lock().ok()?;
        let stale = c.loaded.is_none_or(|t| t.elapsed() > Duration::from_secs(300));
        if stale && !c.loading {
            c.loading = true;
            std::thread::spawn(|| {
                let list = Arc::new(load());
                if let Ok(mut c) = CACHE.lock() {
                    c.list = Some(list);
                    c.loaded = Some(Instant::now());
                    c.loading = false;
                }
            });
        }
        c.list.clone()
    }
}

#[cfg(not(all(target_os = "macos", not(test))))]
mod imp {
    use owlpost_core::Contact;
    use std::sync::Arc;

    pub fn photo(_email: &str) -> Option<Vec<u8>> {
        None
    }
    pub fn people() -> Option<Arc<Vec<Contact>>> {
        None
    }
}
