// ============================================================
//  fetch_redirect_policy.rs — per-Fetch redirect mode registry
// ============================================================

use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::cookie_network::CookieJarRef;
use crate::net::FetchId;

/// Script-visible Fetch redirect mode.
///
/// The value is carried by RequestData, while the registry below transfers the
/// browser-owned decision to the async session redirect layer without putting
/// policy metadata on the wire request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FetchRedirectMode {
    #[default]
    Follow,
    Error,
    Manual,
}

impl FetchRedirectMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            FetchRedirectMode::Follow => "follow",
            FetchRedirectMode::Error => "error",
            FetchRedirectMode::Manual => "manual",
        }
    }
}

/// Cloneable browser-owned redirect policy keyed by FetchId.
#[derive(Clone, Default)]
pub(crate) struct FetchRedirectPolicyRegistry {
    policies: Rc<RefCell<HashMap<FetchId, FetchRedirectMode>>>,
}

impl FetchRedirectPolicyRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set(&self, id: FetchId, mode: FetchRedirectMode) {
        self.policies.borrow_mut().insert(id, mode);
    }

    pub(crate) fn remove(&self, id: FetchId) -> Option<FetchRedirectMode> {
        self.policies.borrow_mut().remove(&id)
    }

    fn downgrade(&self) -> Weak<RefCell<HashMap<FetchId, FetchRedirectMode>>> {
        Rc::downgrade(&self.policies)
    }

    fn ptr_eq(&self, other: &Rc<RefCell<HashMap<FetchId, FetchRedirectMode>>>) -> bool {
        Rc::ptr_eq(&self.policies, other)
    }
}

thread_local! {
    static REDIRECT_POLICIES_BY_JAR: RefCell<
        HashMap<usize, Vec<Weak<RefCell<HashMap<FetchId, FetchRedirectMode>>>>>
    > = RefCell::new(HashMap::new());
}

fn jar_identity(jar: &CookieJarRef) -> usize {
    Rc::as_ptr(jar) as usize
}

fn publish_registry(jar: &CookieJarRef, registry: &FetchRedirectPolicyRegistry) {
    let key = jar_identity(jar);
    REDIRECT_POLICIES_BY_JAR.with(|all| {
        let mut all = all.borrow_mut();
        let stack = all.entry(key).or_default();
        stack.retain(|weak| weak.strong_count() > 0);
        stack.push(registry.downgrade());
    });
}

fn unpublish_registry(jar: &CookieJarRef, registry: &FetchRedirectPolicyRegistry) {
    let key = jar_identity(jar);
    REDIRECT_POLICIES_BY_JAR.with(|all| {
        let mut all = all.borrow_mut();
        let remove_key = if let Some(stack) = all.get_mut(&key) {
            stack.retain(|weak| {
                let Some(candidate) = weak.upgrade() else {
                    return false;
                };
                !registry.ptr_eq(&candidate)
            });
            stack.is_empty()
        } else {
            false
        };
        if remove_key {
            all.remove(&key);
        }
    });
}

/// Lifetime guard that publishes a redirect registry for one Browser session.
pub(crate) struct FetchRedirectPolicyPublication {
    jar: CookieJarRef,
    registry: FetchRedirectPolicyRegistry,
}

impl FetchRedirectPolicyPublication {
    pub(crate) fn new(
        jar: CookieJarRef,
        registry: FetchRedirectPolicyRegistry,
    ) -> FetchRedirectPolicyPublication {
        publish_registry(&jar, &registry);
        FetchRedirectPolicyPublication { jar, registry }
    }
}

impl Drop for FetchRedirectPolicyPublication {
    fn drop(&mut self) {
        unpublish_registry(&self.jar, &self.registry);
    }
}

/// Discover the redirect-policy registry for the Browser session owning `jar`.
/// Standalone Documents and legacy redirect-following transports return None.
pub(crate) fn redirect_policy_registry_for_jar(
    jar: &CookieJarRef,
) -> Option<FetchRedirectPolicyRegistry> {
    let key = jar_identity(jar);
    REDIRECT_POLICIES_BY_JAR.with(|all| {
        let mut all = all.borrow_mut();
        let stack = all.get_mut(&key)?;
        stack.retain(|weak| weak.strong_count() > 0);
        let policies = stack.last()?.upgrade()?;
        Some(FetchRedirectPolicyRegistry { policies })
    })
}
