use std::cell::RefCell;
use std::collections::HashMap;
use std::rc::{Rc, Weak};

use crate::cookie_network::CookieJarRef;
use crate::net::{FetchId, Url};
use crate::referrer_policy::RedirectReferrerState;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchRequestMode {
    Cors,
    SameOrigin,
    NoCors,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FetchCredentialsMode {
    Omit,
    SameOrigin,
    Include,
}

/// Browser-only policy needed to classify every hop of a redirect chain.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FetchCorsRedirectPolicy {
    pub(crate) mode: FetchRequestMode,
    pub(crate) source_url: Url,
    pub(crate) credentials: FetchCredentialsMode,
    pub(crate) referrer: RedirectReferrerState,
}

#[derive(Clone, Default)]
pub(crate) struct FetchCorsRedirectPolicyRegistry {
    policies: Rc<RefCell<HashMap<FetchId, FetchCorsRedirectPolicy>>>,
}

impl FetchCorsRedirectPolicyRegistry {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn set(&self, id: FetchId, policy: FetchCorsRedirectPolicy) {
        self.policies.borrow_mut().insert(id, policy);
    }

    pub(crate) fn remove(&self, id: FetchId) -> Option<FetchCorsRedirectPolicy> {
        self.policies.borrow_mut().remove(&id)
    }

    fn downgrade(&self) -> Weak<RefCell<HashMap<FetchId, FetchCorsRedirectPolicy>>> {
        Rc::downgrade(&self.policies)
    }

    fn ptr_eq(&self, other: &Rc<RefCell<HashMap<FetchId, FetchCorsRedirectPolicy>>>) -> bool {
        Rc::ptr_eq(&self.policies, other)
    }
}

thread_local! {
    static POLICIES_BY_JAR: RefCell<
        HashMap<usize, Vec<Weak<RefCell<HashMap<FetchId, FetchCorsRedirectPolicy>>>>>
    > = RefCell::new(HashMap::new());
}

fn jar_identity(jar: &CookieJarRef) -> usize {
    Rc::as_ptr(jar) as usize
}

fn publish_registry(jar: &CookieJarRef, registry: &FetchCorsRedirectPolicyRegistry) {
    let key = jar_identity(jar);
    POLICIES_BY_JAR.with(|all| {
        let mut all = all.borrow_mut();
        let stack = all.entry(key).or_default();
        stack.retain(|weak| weak.strong_count() > 0);
        stack.push(registry.downgrade());
    });
}

fn unpublish_registry(jar: &CookieJarRef, registry: &FetchCorsRedirectPolicyRegistry) {
    let key = jar_identity(jar);
    POLICIES_BY_JAR.with(|all| {
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

pub(crate) struct FetchCorsRedirectPolicyPublication {
    jar: CookieJarRef,
    registry: FetchCorsRedirectPolicyRegistry,
}

impl FetchCorsRedirectPolicyPublication {
    pub(crate) fn new(
        jar: CookieJarRef,
        registry: FetchCorsRedirectPolicyRegistry,
    ) -> Self {
        publish_registry(&jar, &registry);
        Self { jar, registry }
    }
}

impl Drop for FetchCorsRedirectPolicyPublication {
    fn drop(&mut self) {
        unpublish_registry(&self.jar, &self.registry);
    }
}

pub(crate) fn cors_redirect_policy_registry_for_jar(
    jar: &CookieJarRef,
) -> Option<FetchCorsRedirectPolicyRegistry> {
    let key = jar_identity(jar);
    POLICIES_BY_JAR.with(|all| {
        let mut all = all.borrow_mut();
        let stack = all.get_mut(&key)?;
        stack.retain(|weak| weak.strong_count() > 0);
        let policies = stack.last()?.upgrade()?;
        Some(FetchCorsRedirectPolicyRegistry { policies })
    })
}
