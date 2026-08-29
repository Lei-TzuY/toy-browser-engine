// Public document facade.
//
// The long-lived parser/layout/script implementation remains in `core`, while
// the public Document adds browsing-policy state that belongs to one committed
// document rather than to a redirect chain or a session-global registry.
#[doc(hidden)]
pub mod core {
    include!("document_core.rs");
    include!("document_cookie_session_ext.rs");
    include!("document_subresource_policy_ext.rs");
}

pub use core::{Diagnostic, LoopReport, PageAction, PointerState, UA_STYLESHEET};

use std::collections::HashSet;
use std::ops::{Deref, DerefMut};

use crate::cookie_network::CookieJarRef;
use crate::cookie_same_site::SameSiteRequestContext;
use crate::document_referrer::DocumentReferrerContext;
use crate::navigation_network::NavigationNetwork;
use crate::net::{FetchRequest, FetchResponse, LoadError, Method, ResourceLoader, Url};
use crate::referrer_meta::apply_meta_referrer_policies;
use crate::referrer_policy::ReferrerPolicy;
use crate::script::interp::{EventInit, EventOutcome, StorageRef};
use crate::script::NodePath;

/// One loaded page plus policy state that survives beyond bootstrap.
///
/// `core::Document` intentionally remains the stable rendering/DOM engine. The
/// facade owns the committed document referrer context so later element loads
/// use the exact policy established by the navigation response and parsed
/// metadata instead of reconstructing policy from defaults.
pub struct Document {
    inner: core::Document,
    referrer_context: DocumentReferrerContext,
    /// Failed policy-aware image fetches are remembered independently of the
    /// legacy ImageCache internals, whose public insertion API accepts only
    /// successful decodes. This keeps dynamic refresh from retrying a known
    /// CORS/network/decode failure every time an event mutates the DOM.
    failed_policy_images: HashSet<String>,
}

impl Deref for Document {
    type Target = core::Document;

    fn deref(&self) -> &Self::Target {
        &self.inner
    }
}

impl DerefMut for Document {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.inner
    }
}

impl Document {
    fn context_from_html(html: &str, url: &Url) -> DocumentReferrerContext {
        // Compute metadata policy from the parsed input before authored scripts
        // can mutate the DOM. Standalone documents have no HTTP response
        // header, so the user-agent default is their initial policy.
        let dom = crate::html::parse_html(html);
        let policy = apply_meta_referrer_policies(&dom, ReferrerPolicy::default());
        DocumentReferrerContext::new(Some(url.clone()), policy)
    }

    fn wrap(inner: core::Document, referrer_context: DocumentReferrerContext) -> Document {
        Document {
            inner,
            referrer_context,
            failed_policy_images: HashSet::new(),
        }
    }

    /// Fetch `url` and everything it references.
    pub fn load(url: &Url, loader: &dyn ResourceLoader) -> Result<Document, LoadError> {
        Self::load_with_storage(url, loader, None)
    }

    /// Fetch a standalone document with caller-supplied persistent storage.
    pub fn load_with_storage(
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<StorageRef>,
    ) -> Result<Document, LoadError> {
        let resource = loader.load(url)?;
        Ok(Self::from_html_with_storage(
            &resource.text(),
            &resource.url,
            loader,
            storage,
        ))
    }

    /// Build a standalone document from already-fetched HTML.
    pub fn from_html(html: &str, url: &Url, loader: &dyn ResourceLoader) -> Document {
        Self::from_html_with_storage(html, url, loader, None)
    }

    /// Build a standalone document and retain its parsed referrer metadata.
    pub fn from_html_with_storage(
        html: &str,
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<StorageRef>,
    ) -> Document {
        let referrer_context = Self::context_from_html(html, url);
        let inner = core::Document::from_html_with_storage(html, url, loader, storage);
        Self::wrap(inner, referrer_context)
    }

    /// Fetch a document while installing caller-owned session state before
    /// authored scripts execute.
    pub fn load_with_session_state(
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<StorageRef>,
        cookie_jar: Option<CookieJarRef>,
    ) -> Result<Document, LoadError> {
        let resource = loader.load(url)?;
        Ok(Self::from_html_with_session_state(
            &resource.text(),
            &resource.url,
            loader,
            storage,
            cookie_jar,
        ))
    }

    /// Build a standalone/session document with shared script state while
    /// retaining its initial metadata-selected referrer policy.
    pub fn from_html_with_session_state(
        html: &str,
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<StorageRef>,
        cookie_jar: Option<CookieJarRef>,
    ) -> Document {
        let referrer_context = Self::context_from_html(html, url);
        let inner = core::Document::from_html_with_session_state(
            html,
            url,
            loader,
            storage,
            cookie_jar,
        );
        Self::wrap(inner, referrer_context)
    }

    /// Build a browser-session document from the final navigation response.
    ///
    /// The exact committed response policy is captured independently of the
    /// synchronous bootstrap implementation. Parsing here is intentional: it
    /// freezes response-header + parser-time `<meta name=referrer>` state before
    /// authored scripts get an opportunity to rewrite those elements.
    pub fn from_response_with_session_subresources(
        response: &FetchResponse,
        navigation: &NavigationNetwork,
        storage: Option<StorageRef>,
        cookie_jar: Option<CookieJarRef>,
    ) -> Document {
        let html = String::from_utf8_lossy(&response.body);
        let parsed = crate::html::parse_html(&html);
        let referrer_context =
            DocumentReferrerContext::from_response_and_document(response, &parsed);
        let inner = core::Document::from_response_with_session_subresources(
            response,
            navigation,
            storage,
            cookie_jar,
        );
        Self::wrap(inner, referrer_context)
    }

    /// Referrer source and policy owned by this committed document.
    pub fn referrer_context(&self) -> &DocumentReferrerContext {
        &self.referrer_context
    }

    /// Dispatch through the runtime while borrowing `runtime` and `dom` as
    /// disjoint fields of the inner core document.
    ///
    /// Callers that only see the facade cannot express this split borrow via
    /// DerefMut: the dereference itself borrows the entire facade. Keeping the
    /// operation here preserves the safe field-level borrow the core type had.
    pub(crate) fn dispatch_runtime_event(
        &mut self,
        path: &NodePath,
        event_type: &str,
    ) -> EventOutcome {
        self.inner
            .runtime
            .dispatch_event(&mut self.inner.dom, path, event_type)
    }

    /// Event-dispatch variant with an explicit event initializer.
    pub(crate) fn dispatch_runtime_event_init(
        &mut self,
        path: &NodePath,
        event_type: &str,
        init: EventInit,
    ) -> EventOutcome {
        self.inner
            .runtime
            .dispatch_event_init(&mut self.inner.dom, path, event_type, init)
    }

    /// Refresh images added after bootstrap using the committed document policy.
    ///
    /// This intentionally shadows the compatibility method on `core::Document`.
    /// The old core method has no place to store a final response header and
    /// therefore reconstructed the user-agent default. The facade can instead
    /// carry the exact committed context through every later image request.
    pub fn refresh_images_with_subresource_policy(&mut self, navigation: &NavigationNetwork) {
        for source in facade_image_sources(&self.inner.dom) {
            let Ok(url) = self.inner.base_url.join(&source.src) else {
                self.inner.diagnostics.push(Diagnostic {
                    url: source.src,
                    message: "could not resolve image URL".into(),
                });
                continue;
            };
            let key = url.without_fragment().to_string();
            if self.inner.images.get(&url).is_some()
                || self.inner.images.error(&url).is_some()
                || self.failed_policy_images.contains(&key)
            {
                continue;
            }

            let effective_target = navigation.effective_url(&url);
            let same_site = self.referrer_context.source().is_some_and(|source| {
                source.scheme() == effective_target.scheme()
                    && source.host().eq_ignore_ascii_case(effective_target.host())
            });
            let context = SameSiteRequestContext::new(same_site, false, Method::Get);
            let request = FetchRequest::get(url.clone());
            let outcome = self
                .referrer_context
                .fetch_subresource_with_cors_credentials(
                    navigation,
                    &request,
                    context,
                    source.referrerpolicy.as_deref(),
                    source.crossorigin.as_deref(),
                )
                .map_err(|error| error.to_string())
                .and_then(|response| {
                    if response.ok() {
                        crate::image::decode(&response.body)
                            .map_err(|error| format!("{url}: {error}"))
                    } else {
                        Err(format!("HTTP {} at {}", response.status, response.url))
                    }
                });

            match outcome {
                Ok(image) => self.inner.images.insert(&url, image),
                Err(message) => {
                    self.failed_policy_images.insert(key);
                    self.inner.diagnostics.push(Diagnostic {
                        url: url.to_string(),
                        message,
                    });
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
struct FacadeImageSource {
    src: String,
    crossorigin: Option<String>,
    referrerpolicy: Option<String>,
}

fn facade_image_sources(dom: &crate::dom::Node) -> Vec<FacadeImageSource> {
    fn walk(node: &crate::dom::Node, out: &mut Vec<FacadeImageSource>) {
        if let crate::dom::NodeType::Element(element) = &node.node_type {
            if element.tag_name == "img" {
                if let Some(src) = element.get_attr("src") {
                    if !src.trim().is_empty() {
                        out.push(FacadeImageSource {
                            src: src.to_string(),
                            crossorigin: element.get_attr("crossorigin").map(str::to_string),
                            referrerpolicy: element.get_attr("referrerpolicy").map(str::to_string),
                        });
                    }
                }
            }
        }
        for child in &node.children {
            walk(child, out);
        }
    }

    let mut out = Vec::new();
    walk(dom, &mut out);
    out
}
