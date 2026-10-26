use std::cell::RefCell;
use std::rc::Rc;

use browser_engine::cookie::CookieJar;
use browser_engine::cookie_network::{
    policy_registry_for_jar, CookieCredentials, CookieJarRef, CookieNetwork,
    CookiePolicyRegistry, CookieRequestPolicy,
};
use browser_engine::cookie_same_site::SameSiteRequestContext;
use browser_engine::eventloop::ManualClock;
use browser_engine::net::fetch::Method;
use browser_engine::net::{FetchRequest, ManualNetwork, NetworkBackend, Url};

fn jar() -> CookieJarRef {
    Rc::new(RefCell::new(CookieJar::new()))
}

fn omit() -> CookieRequestPolicy {
    CookieRequestPolicy::omit(SameSiteRequestContext::same_site(Method::Get))
}

fn include() -> CookieRequestPolicy {
    CookieRequestPolicy::new(
        CookieCredentials::Include,
        SameSiteRequestContext::same_site(Method::Get),
    )
}

#[test]
fn ordinary_cookie_jar_has_no_network_policy_registry() {
    let standalone = jar();
    assert!(policy_registry_for_jar(&standalone).is_none());
}

#[test]
fn cookie_network_publishes_the_exact_registry_for_its_session_jar() {
    let session = jar();
    let network = CookieNetwork::new(
        Rc::new(ManualNetwork::new()),
        session.clone(),
        Rc::new(ManualClock::new()),
    );
    let direct = network.policy_registry();
    let discovered = policy_registry_for_jar(&session).expect("published registry");

    assert_eq!(direct.len(), 0);
    discovered.set(7, omit());
    assert_eq!(direct.get(7), Some(omit()));
    assert_eq!(network.request_policy(7), Some(omit()));
}

#[test]
fn discovered_registry_controls_the_actual_cookie_network_request() {
    let inner = Rc::new(ManualNetwork::new());
    let session = jar();
    let source = Url::parse("https://example.test/").unwrap();
    assert!(session
        .borrow_mut()
        .store_set_cookie("sid=secret; Path=/", &source, 0));

    let network = CookieNetwork::new(
        inner.clone(),
        session.clone(),
        Rc::new(ManualClock::new()),
    );
    let registry = policy_registry_for_jar(&session).expect("published registry");
    registry.set(11, omit());

    network.start(
        11,
        FetchRequest::get(Url::parse("https://example.test/api").unwrap()),
    );

    assert_eq!(inner.requests().len(), 1);
    assert!(inner.requests()[0].headers.get("cookie").is_none());
    assert_eq!(network.pending_policy_count(), 1);
}

#[test]
fn dropping_cookie_network_unpublishes_its_registry_even_if_a_handle_survives() {
    let session = jar();
    let surviving_handle = {
        let network = CookieNetwork::new(
            Rc::new(ManualNetwork::new()),
            session.clone(),
            Rc::new(ManualClock::new()),
        );
        let handle = network.policy_registry();
        assert!(policy_registry_for_jar(&session).is_some());
        handle
    };

    assert!(
        policy_registry_for_jar(&session).is_none(),
        "session discovery must describe a live CookieNetwork, not merely a surviving map"
    );
    surviving_handle.set(1, include());
    assert_eq!(surviving_handle.get(1), Some(include()));
}

#[test]
fn nested_cookie_networks_over_one_jar_restore_the_previous_registry_on_drop() {
    let session = jar();
    let first = CookieNetwork::new(
        Rc::new(ManualNetwork::new()),
        session.clone(),
        Rc::new(ManualClock::new()),
    );
    let first_registry = first.policy_registry();

    {
        let second = CookieNetwork::new(
            Rc::new(ManualNetwork::new()),
            session.clone(),
            Rc::new(ManualClock::new()),
        );
        let second_registry = second.policy_registry();
        let discovered = policy_registry_for_jar(&session).expect("newest registry");
        discovered.set(20, omit());
        assert_eq!(second_registry.get(20), Some(omit()));
        assert_eq!(first_registry.get(20), None);
    }

    let restored = policy_registry_for_jar(&session).expect("older registry restored");
    restored.set(21, include());
    assert_eq!(first_registry.get(21), Some(include()));
}

#[test]
fn different_cookie_jars_never_share_request_policy() {
    let first_jar = jar();
    let second_jar = jar();
    let _first = CookieNetwork::new(
        Rc::new(ManualNetwork::new()),
        first_jar.clone(),
        Rc::new(ManualClock::new()),
    );
    let _second = CookieNetwork::new(
        Rc::new(ManualNetwork::new()),
        second_jar.clone(),
        Rc::new(ManualClock::new()),
    );

    let first = policy_registry_for_jar(&first_jar).unwrap();
    let second = policy_registry_for_jar(&second_jar).unwrap();
    first.set(30, omit());

    assert_eq!(first.get(30), Some(omit()));
    assert_eq!(second.get(30), None);
}

#[test]
fn caller_supplied_registry_is_the_one_cookie_network_publishes() {
    let session = jar();
    let supplied = CookiePolicyRegistry::new();
    supplied.set(40, omit());

    let network = CookieNetwork::with_policy_registry(
        Rc::new(ManualNetwork::new()),
        session.clone(),
        Rc::new(ManualClock::new()),
        supplied.clone(),
    );
    let discovered = policy_registry_for_jar(&session).unwrap();

    assert_eq!(network.request_policy(40), Some(omit()));
    assert_eq!(discovered.get(40), Some(omit()));
    discovered.remove(40);
    assert!(supplied.is_empty());
}
