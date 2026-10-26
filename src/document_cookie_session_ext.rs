// ============================================================
// document_cookie_session_ext.rs — shared session state at bootstrap
// ============================================================

impl Document {
    /// Fetch a document while installing caller-owned persistent state before
    /// any authored script executes.
    ///
    /// This is the Browser/session-oriented counterpart to
    /// [`Document::load_with_storage`]. Keeping the cookie jar optional
    /// preserves the standalone document API while letting a Browser share one
    /// jar across navigation, reload, Fetch and `document.cookie`.
    pub fn load_with_session_state(
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<crate::script::interp::StorageRef>,
        cookie_jar: Option<crate::cookie_network::CookieJarRef>,
    ) -> Result<Document, LoadError> {
        let resource = loader.load(url)?;
        Ok(Document::from_html_with_session_state(
            &resource.text(),
            &resource.url,
            loader,
            storage,
            cookie_jar,
        ))
    }

    /// Build a document from fetched HTML with persistent session state already
    /// attached to the JavaScript runtime when the first `<script>` runs.
    pub fn from_html_with_session_state(
        html: &str,
        url: &Url,
        loader: &dyn ResourceLoader,
        storage: Option<crate::script::interp::StorageRef>,
        cookie_jar: Option<crate::cookie_network::CookieJarRef>,
    ) -> Document {
        let mut dom = crate::html::parse_html(html);
        let base_url = base_url_of(&dom, url);
        // No session URL here: the page's own directory is its local root.
        let requester = Requester::new(url, None);
        let mut diagnostics = Vec::new();

        seed_textarea_values(&mut dom);

        let subresources = SubresourceLoader::new(loader, &requester);
        let stylesheet = collect_stylesheet(&dom, &base_url, &subresources, &mut diagnostics);
        let runtime = run_scripts_with_session_state(
            &mut dom,
            url,
            &base_url,
            &subresources,
            &mut diagnostics,
            storage,
            cookie_jar,
        );

        let mut document = Document {
            url: url.clone(),
            base_url,
            dom,
            stylesheet,
            runtime,
            images: ImageCache::new(),
            requester,
            diagnostics,
            focus: None,
            deferred_submission: None,
            network: Rc::new(OfflineNetwork::new()),
            transitions: std::cell::RefCell::new(crate::transition::TransitionManager::new()),
            animations: std::cell::RefCell::new(crate::animation::AnimationManager::new()),
        };
        document.run_microtask_checkpoint();
        document.refresh_images(loader);
        document
    }
}

/// The core bootstrap path creates a fresh runtime. This variant installs the
/// shared session handles immediately after creation and before executing the
/// pre-collected document-order script list.
fn run_scripts_with_session_state(
    dom: &mut Node,
    document_url: &Url,
    base_url: &Url,
    loader: &dyn ResourceLoader,
    diagnostics: &mut Vec<Diagnostic>,
    storage: Option<crate::script::interp::StorageRef>,
    cookie_jar: Option<crate::cookie_network::CookieJarRef>,
) -> JsRuntime {
    let sources = script_sources(dom);
    let mut runtime = JsRuntime::for_document(document_url.clone(), base_url.clone());
    if let Some(storage) = storage {
        runtime.local_storage = storage;
    }
    if let Some(cookie_jar) = cookie_jar {
        runtime.cookie_jar = cookie_jar;
    }

    for source in sources {
        match source {
            ScriptSource::Inline(code) => runtime.run_script(dom, &code),
            ScriptSource::External(src) => {
                let Ok(url) = base_url.join(&src) else {
                    diagnostics.push(Diagnostic {
                        url: src.clone(),
                        message: "could not resolve script URL".into(),
                    });
                    continue;
                };
                match loader.load(&url) {
                    Ok(resource) => runtime.run_script(dom, &resource.text()),
                    Err(error) => diagnostics.push(Diagnostic {
                        url: url.to_string(),
                        message: error.to_string(),
                    }),
                }
            }
        }
    }
    runtime
}
