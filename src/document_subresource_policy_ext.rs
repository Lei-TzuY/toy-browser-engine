// ============================================================
// document_subresource_policy_ext.rs — bootstrap element fetch policy
// ============================================================

const MAX_DYNAMIC_ELEMENT_SUBRESOURCES_PER_REFRESH: usize = 64;

/// Browser/session bootstrap path for document-owned subresources.
///
/// The older standalone Document constructors deliberately keep their simple
/// ResourceLoader behavior for embedders. Browser uses this constructor after a
/// top-level response has passed navigation policy, so linked stylesheets,
/// external scripts and images can carry the committed document's referrer,
/// CORS and credential state into the synchronous subresource fetch path.
impl Document {
    pub fn from_response_with_session_subresources(
        response: &crate::net::FetchResponse,
        navigation: &crate::navigation_network::NavigationNetwork,
        storage: Option<crate::script::interp::StorageRef>,
        cookie_jar: Option<crate::cookie_network::CookieJarRef>,
    ) -> Document {
        let html = String::from_utf8_lossy(&response.body);
        let mut dom = crate::html::parse_html(&html);
        let base_url = base_url_of(&dom, &response.url);
        let mut diagnostics = Vec::new();

        seed_textarea_values(&mut dom);

        // The final navigation response establishes the initial policy and
        // parsed <meta name=referrer> elements then update it in tree order.
        let referrer = crate::document_referrer::DocumentReferrerContext::from_response_and_document(
            response,
            &dom,
        );

        let stylesheet = collect_stylesheet_with_subresource_policy(
            &dom,
            &base_url,
            navigation,
            &referrer,
            &mut diagnostics,
        );
        // Parser-time stylesheet links were prepared by the collection pass.
        // Freeze that fact before scripts can add new links to the live tree.
        mark_existing_stylesheet_links_started(&mut dom);

        let runtime = run_scripts_with_subresource_policy(
            &mut dom,
            &base_url,
            navigation,
            &referrer,
            &mut diagnostics,
            storage,
            cookie_jar,
        );

        let mut document = Document {
            url: response.url.clone(),
            base_url,
            dom,
            stylesheet,
            runtime,
            images: ImageCache::new(),
            diagnostics,
            focus: None,
            deferred_submission: None,
            network: Rc::new(OfflineNetwork::new()),
            transitions: std::cell::RefCell::new(crate::transition::TransitionManager::new()),
            animations: std::cell::RefCell::new(crate::animation::AnimationManager::new()),
        };
        document.run_microtask_checkpoint();
        // Parser-time scripts and their microtasks may have appended external
        // scripts or stylesheet links. Activate those before the first paint,
        // then load images produced by the whole settled chain.
        document.refresh_dynamic_element_subresources_with_referrer_context(navigation, &referrer);
        document
    }

    /// Refresh images added by script without falling back to the raw loader.
    ///
    /// A Document does not yet persist the response-header referrer policy as a
    /// field, so later DOM refreshes reconstruct the live policy from the
    /// modern default plus current meta elements. Bootstrap loads above still
    /// use the exact final response header. CORS and credential behavior remain
    /// fully enforced for dynamic images in either case.
    pub fn refresh_images_with_subresource_policy(
        &mut self,
        navigation: &crate::navigation_network::NavigationNetwork,
    ) {
        let policy = crate::referrer_meta::apply_meta_referrer_policies(
            &self.dom,
            crate::referrer_policy::ReferrerPolicy::default(),
        );
        let referrer = crate::document_referrer::DocumentReferrerContext::new(
            Some(self.url.clone()),
            policy,
        );
        self.refresh_images_with_referrer_context(navigation, &referrer);
    }

    /// Activate script/link elements added after parsing, in current document
    /// order, then refresh images produced by those scripts.
    ///
    /// Script/link preparation is stored on the element itself rather than in
    /// a URL cache: two distinct elements referencing one URL are two loads,
    /// while removing and re-inserting the same script node cannot execute it
    /// twice. Each fetch reuses the same CORS/credentials/referrer/HSTS path as
    /// parser-time element fetches.
    pub(crate) fn refresh_dynamic_element_subresources_with_referrer_context(
        &mut self,
        navigation: &crate::navigation_network::NavigationNetwork,
        referrer: &crate::document_referrer::DocumentReferrerContext,
    ) {
        // Bound the number we *take* from the live DOM rather than taking one
        // extra and then checking the budget. `take_next...` marks an element
        // before returning it, so this order is what leaves item 65 untouched
        // and eligible for a later refresh instead of silently losing it.
        for _ in 0..MAX_DYNAMIC_ELEMENT_SUBRESOURCES_PER_REFRESH {
            let Some(source) = take_next_dynamic_element_subresource(&mut self.dom) else {
                break;
            };

            match source {
                DynamicElementSubresource::Stylesheet {
                    href,
                    crossorigin,
                    referrerpolicy,
                } => {
                    let Ok(url) = self.base_url.join(&href) else {
                        self.diagnostics.push(Diagnostic {
                            url: href,
                            message: "could not resolve stylesheet URL".into(),
                        });
                        continue;
                    };
                    match fetch_document_subresource(
                        navigation,
                        referrer,
                        &url,
                        crossorigin.as_deref(),
                        referrerpolicy.as_deref(),
                    ) {
                        Ok(response) => {
                            let css = String::from_utf8_lossy(&response.body);
                            let parsed = parse_css(&css);
                            self.stylesheet.rules.extend(parsed.rules);
                            self.stylesheet.keyframes.extend(parsed.keyframes);
                        }
                        Err(message) => self.diagnostics.push(Diagnostic {
                            url: url.to_string(),
                            message,
                        }),
                    }
                }
                DynamicElementSubresource::Script {
                    src,
                    crossorigin,
                    referrerpolicy,
                } => {
                    let Ok(url) = self.base_url.join(&src) else {
                        self.diagnostics.push(Diagnostic {
                            url: src,
                            message: "could not resolve script URL".into(),
                        });
                        continue;
                    };
                    match fetch_document_subresource(
                        navigation,
                        referrer,
                        &url,
                        crossorigin.as_deref(),
                        referrerpolicy.as_deref(),
                    ) {
                        Ok(response) => {
                            let code = String::from_utf8_lossy(&response.body);
                            self.runtime.run_script(&mut self.dom, &code);
                            // A dynamically loaded script is a script task too:
                            // settle its promise callbacks before preparing the
                            // next element that it may just have inserted.
                            self.run_microtask_checkpoint();
                        }
                        Err(message) => self.diagnostics.push(Diagnostic {
                            url: url.to_string(),
                            message,
                        }),
                    }
                }
            }
        }

        self.refresh_images_with_referrer_context(navigation, referrer);
    }

    fn refresh_images_with_referrer_context(
        &mut self,
        navigation: &crate::navigation_network::NavigationNetwork,
        referrer: &crate::document_referrer::DocumentReferrerContext,
    ) {
        for source in policy_image_sources(&self.dom) {
            let Ok(url) = self.base_url.join(&source.src) else {
                self.diagnostics.push(Diagnostic {
                    url: source.src,
                    message: "could not resolve image URL".into(),
                });
                continue;
            };
            if self.images.get(&url).is_some() || self.images.error(&url).is_some() {
                continue;
            }

            match fetch_document_subresource(
                navigation,
                referrer,
                &url,
                source.crossorigin.as_deref(),
                source.referrerpolicy.as_deref(),
            ) {
                Ok(response) => match crate::image::decode(&response.body) {
                    Ok(image) => self.images.insert(&url, image),
                    Err(error) => {
                        let message = format!("{url}: {error}");
                        self.images.insert_error(&url, message.clone());
                        self.diagnostics.push(Diagnostic {
                            url: url.to_string(),
                            message,
                        });
                    }
                },
                Err(message) => {
                    self.images.insert_error(&url, message.clone());
                    self.diagnostics.push(Diagnostic {
                        url: url.to_string(),
                        message,
                    });
                }
            }
        }
    }
}

#[derive(Debug, Clone)]
enum PolicyStyleSource {
    Inline(String),
    Link {
        href: String,
        crossorigin: Option<String>,
        referrerpolicy: Option<String>,
    },
}

fn collect_stylesheet_with_subresource_policy(
    dom: &Node,
    base_url: &Url,
    navigation: &crate::navigation_network::NavigationNetwork,
    referrer: &crate::document_referrer::DocumentReferrerContext,
    diagnostics: &mut Vec<Diagnostic>,
) -> Stylesheet {
    let mut stylesheet = parse_css(UA_STYLESHEET);

    for source in policy_style_sources(dom) {
        match source {
            PolicyStyleSource::Inline(css) => {
                let parsed = parse_css(&css);
                stylesheet.rules.extend(parsed.rules);
                stylesheet.keyframes.extend(parsed.keyframes);
            }
            PolicyStyleSource::Link {
                href,
                crossorigin,
                referrerpolicy,
            } => {
                let Ok(url) = base_url.join(&href) else {
                    diagnostics.push(Diagnostic {
                        url: href,
                        message: "could not resolve stylesheet URL".into(),
                    });
                    continue;
                };
                match fetch_document_subresource(
                    navigation,
                    referrer,
                    &url,
                    crossorigin.as_deref(),
                    referrerpolicy.as_deref(),
                ) {
                    Ok(response) => {
                        let css = String::from_utf8_lossy(&response.body);
                        let parsed = parse_css(&css);
                        stylesheet.rules.extend(parsed.rules);
                        stylesheet.keyframes.extend(parsed.keyframes);
                    }
                    Err(message) => diagnostics.push(Diagnostic {
                        url: url.to_string(),
                        message,
                    }),
                }
            }
        }
    }

    stylesheet
}

fn policy_style_sources(dom: &Node) -> Vec<PolicyStyleSource> {
    fn walk(node: &Node, out: &mut Vec<PolicyStyleSource>) {
        if let NodeType::Element(element) = &node.node_type {
            match element.tag_name.as_str() {
                "style" => {
                    let css: String = node
                        .children
                        .iter()
                        .filter_map(|child| match &child.node_type {
                            NodeType::Text(text) => Some(text.as_str()),
                            _ => None,
                        })
                        .collect();
                    out.push(PolicyStyleSource::Inline(css));
                    return;
                }
                "link" => {
                    let rel = element
                        .get_attr("rel")
                        .unwrap_or_default()
                        .to_ascii_lowercase();
                    if rel.split_whitespace().any(|token| token == "stylesheet") {
                        if let Some(href) = element.get_attr("href") {
                            out.push(PolicyStyleSource::Link {
                                href: href.to_string(),
                                crossorigin: element.get_attr("crossorigin").map(str::to_string),
                                referrerpolicy: element
                                    .get_attr("referrerpolicy")
                                    .map(str::to_string),
                            });
                        }
                    }
                    return;
                }
                _ => {}
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

fn mark_existing_stylesheet_links_started(dom: &mut Node) {
    fn walk(node: &mut Node) {
        if let NodeType::Element(element) = &mut node.node_type {
            if element.tag_name == "link" {
                let is_stylesheet = element
                    .get_attr("rel")
                    .unwrap_or_default()
                    .split_ascii_whitespace()
                    .any(|token| token.eq_ignore_ascii_case("stylesheet"));
                if is_stylesheet && element.get_attr("href").is_some() {
                    element.start_stylesheet_once();
                }
                return;
            }
        }
        for child in &mut node.children {
            walk(child);
        }
    }
    walk(dom);
}

#[derive(Debug, Clone)]
enum PolicyScriptSource {
    Inline(String),
    External {
        src: String,
        crossorigin: Option<String>,
        referrerpolicy: Option<String>,
    },
}

fn run_scripts_with_subresource_policy(
    dom: &mut Node,
    base_url: &Url,
    navigation: &crate::navigation_network::NavigationNetwork,
    referrer: &crate::document_referrer::DocumentReferrerContext,
    diagnostics: &mut Vec<Diagnostic>,
    storage: Option<crate::script::interp::StorageRef>,
    cookie_jar: Option<crate::cookie_network::CookieJarRef>,
) -> JsRuntime {
    // Snapshot parser-time sources, then mark every parser-created script as
    // already prepared before execution. Scripts created by those scripts are
    // therefore distinguishable later without exposing an internal attribute.
    let sources = policy_script_sources(dom);
    mark_existing_scripts_started(dom);

    let mut runtime = JsRuntime::new();
    runtime.url = base_url.clone();
    if let Some(storage) = storage {
        runtime.local_storage = storage;
    }
    if let Some(cookie_jar) = cookie_jar {
        runtime.cookie_jar = cookie_jar;
    }

    for source in sources {
        match source {
            PolicyScriptSource::Inline(code) => runtime.run_script(dom, &code),
            PolicyScriptSource::External {
                src,
                crossorigin,
                referrerpolicy,
            } => {
                let Ok(url) = base_url.join(&src) else {
                    diagnostics.push(Diagnostic {
                        url: src,
                        message: "could not resolve script URL".into(),
                    });
                    continue;
                };
                match fetch_document_subresource(
                    navigation,
                    referrer,
                    &url,
                    crossorigin.as_deref(),
                    referrerpolicy.as_deref(),
                ) {
                    Ok(response) => {
                        let code = String::from_utf8_lossy(&response.body);
                        runtime.run_script(dom, &code);
                    }
                    Err(message) => diagnostics.push(Diagnostic {
                        url: url.to_string(),
                        message,
                    }),
                }
            }
        }
    }

    runtime
}

fn mark_existing_scripts_started(dom: &mut Node) {
    fn walk(node: &mut Node) {
        if let NodeType::Element(element) = &mut node.node_type {
            if element.tag_name == "script" {
                element.start_script_once();
                return;
            }
        }
        for child in &mut node.children {
            walk(child);
        }
    }
    walk(dom);
}

fn policy_script_sources(dom: &Node) -> Vec<PolicyScriptSource> {
    fn walk(node: &Node, out: &mut Vec<PolicyScriptSource>) {
        if let NodeType::Element(element) = &node.node_type {
            if element.tag_name == "script" {
                match element.get_attr("src") {
                    Some(src) if !src.trim().is_empty() => {
                        out.push(PolicyScriptSource::External {
                            src: src.to_string(),
                            crossorigin: element.get_attr("crossorigin").map(str::to_string),
                            referrerpolicy: element.get_attr("referrerpolicy").map(str::to_string),
                        });
                    }
                    _ => {
                        let code: String = node
                            .children
                            .iter()
                            .filter_map(|child| match &child.node_type {
                                NodeType::Text(text) => Some(text.as_str()),
                                _ => None,
                            })
                            .collect();
                        if !code.trim().is_empty() {
                            out.push(PolicyScriptSource::Inline(code));
                        }
                    }
                }
                return;
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

#[derive(Debug, Clone)]
enum DynamicElementSubresource {
    Stylesheet {
        href: String,
        crossorigin: Option<String>,
        referrerpolicy: Option<String>,
    },
    Script {
        src: String,
        crossorigin: Option<String>,
        referrerpolicy: Option<String>,
    },
}

/// Take and mark the next unprepared dynamic script/link in live document
/// order. Marking happens before URL or network work so failures are one-shot.
fn take_next_dynamic_element_subresource(dom: &mut Node) -> Option<DynamicElementSubresource> {
    fn walk(node: &mut Node) -> Option<DynamicElementSubresource> {
        if let NodeType::Element(element) = &mut node.node_type {
            match element.tag_name.as_str() {
                "script" => {
                    let src = element.get_attr("src")?.trim().to_string();
                    if !src.is_empty() && element.start_script_once() {
                        return Some(DynamicElementSubresource::Script {
                            src,
                            crossorigin: element.get_attr("crossorigin").map(str::to_string),
                            referrerpolicy: element
                                .get_attr("referrerpolicy")
                                .map(str::to_string),
                        });
                    }
                    return None;
                }
                "link" => {
                    let is_stylesheet = element
                        .get_attr("rel")
                        .unwrap_or_default()
                        .split_ascii_whitespace()
                        .any(|token| token.eq_ignore_ascii_case("stylesheet"));
                    if is_stylesheet {
                        if let Some(href) = element.get_attr("href").map(str::to_string) {
                            if element.start_stylesheet_once() {
                                return Some(DynamicElementSubresource::Stylesheet {
                                    href,
                                    crossorigin: element
                                        .get_attr("crossorigin")
                                        .map(str::to_string),
                                    referrerpolicy: element
                                        .get_attr("referrerpolicy")
                                        .map(str::to_string),
                                });
                            }
                        }
                    }
                    return None;
                }
                _ => {}
            }
        }
        for child in &mut node.children {
            if let Some(source) = walk(child) {
                return Some(source);
            }
        }
        None
    }

    walk(dom)
}

#[derive(Debug, Clone)]
struct PolicyImageSource {
    src: String,
    crossorigin: Option<String>,
    referrerpolicy: Option<String>,
}

fn policy_image_sources(dom: &Node) -> Vec<PolicyImageSource> {
    fn walk(node: &Node, out: &mut Vec<PolicyImageSource>) {
        if let NodeType::Element(element) = &node.node_type {
            if element.tag_name == "img" {
                if let Some(src) = element.get_attr("src") {
                    if !src.trim().is_empty() {
                        out.push(PolicyImageSource {
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

fn fetch_document_subresource(
    navigation: &crate::navigation_network::NavigationNetwork,
    referrer: &crate::document_referrer::DocumentReferrerContext,
    url: &Url,
    crossorigin: Option<&str>,
    referrerpolicy: Option<&str>,
) -> Result<crate::net::FetchResponse, String> {
    let effective_target = navigation.effective_url(url);
    let same_site = referrer.source().is_some_and(|source| {
        source.scheme() == effective_target.scheme()
            && source.host().eq_ignore_ascii_case(effective_target.host())
    });
    let context = crate::cookie_same_site::SameSiteRequestContext::new(
        same_site,
        false,
        crate::net::Method::Get,
    );
    let request = crate::net::FetchRequest::get(url.clone());
    let response = referrer
        .fetch_subresource_with_cors_credentials(
            navigation,
            &request,
            context,
            referrerpolicy,
            crossorigin,
        )
        .map_err(|error| error.to_string())?;

    if response.ok() {
        Ok(response)
    } else {
        Err(format!("HTTP {} at {}", response.status, response.url))
    }
}
