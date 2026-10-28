// ============================================================
//  tabs.rs  —  independent browsing tabs for --window
// ============================================================
//
//  A tab is one `Browser` session plus the window chrome state that belongs
//  to it: its toolbar (address text, editing focus, last error) and its
//  page scroll offset. Every tab owns its own session, so documents,
//  history, cookies, HSTS, localStorage and the fetch network are never
//  shared between tabs; each tab goes through the same `Browser` navigation
//  and security stack a single-page window always used.
//
//  `Tabs` is the ordered list with one active entry. `main` routes all
//  input to the active tab and paints only that one.

use browser_engine::{
    browser::{Browser, ClickOutcome},
    input::KeyEvent,
    net::{url_from_argument, LoadError},
};

use crate::toolbar::{Command, Navigation, Toolbar};

/// Something the tab strip or a tab shortcut wants done.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TabCommand {
    /// Ctrl+T or the "+" button.
    New,
    /// Ctrl+W, a close button or a middle click.
    Close(usize),
    /// A click on a tab.
    Select(usize),
    /// Ctrl+Tab.
    Next,
    /// Ctrl+Shift+Tab.
    Previous,
    /// Ctrl+1 … Ctrl+9: the nth tab, except that 9 is always the last.
    Number(usize),
}

pub(crate) struct Tab {
    pub browser: Browser,
    pub toolbar: Toolbar,
    pub scroll_y: f32,
}

impl Tab {
    pub(crate) fn new(browser: Browser, width: usize) -> Tab {
        Tab {
            browser,
            toolbar: Toolbar::new(width),
            scroll_y: 0.0,
        }
    }

    /// What the tab strip shows: the page title, or its URL without one.
    pub(crate) fn label(&self) -> String {
        self.browser
            .document()
            .title()
            .unwrap_or_else(|| self.browser.url().to_string())
    }

    /// Title-bar text for this tab, led by its address-field error if any.
    pub(crate) fn status(&self) -> String {
        match self.toolbar.error() {
            Some(error) => format!("{error} — {}", self.browser.status_line()),
            None => self.browser.status_line(),
        }
    }

    /// Hand a frame's keystrokes to this tab: the address field takes them
    /// while focused, otherwise they go to the page (unless `chrome_only`,
    /// for Alt shortcuts). Returns the toolbar command they amount to.
    pub(crate) fn handle_keys(
        &mut self,
        events: Vec<KeyEvent>,
        chrome_only: bool,
    ) -> Option<Command> {
        let dispatch = self.toolbar.dispatch_keys(events);
        if !chrome_only {
            for event in dispatch.page {
                let outcome = self.browser.press_key(&event);
                self.report_page_outcome(outcome);
            }
        }
        dispatch.command
    }

    /// Carry out a toolbar button, Alt shortcut or address-field navigation.
    pub(crate) fn run_command(&mut self, command: Command) {
        match command {
            Command::Back => {
                if self.browser.back() {
                    self.scroll_y = 0.0;
                }
            }
            Command::Forward => {
                if self.browser.forward() {
                    self.scroll_y = 0.0;
                }
            }
            Command::Reload => {
                if let Err(error) = self.browser.reload() {
                    eprintln!("reload failed: {error}");
                }
            }
            Command::Navigate(target) => {
                let url = url_from_argument(&target);
                match self.browser.navigate(&url) {
                    Ok(()) => {
                        println!("→ {}", self.browser.url());
                        self.scroll_y = 0.0;
                        self.toolbar.navigation_succeeded();
                    }
                    Err(error) => {
                        // The previous page stays on screen and the typed text
                        // stays in the field for correction.
                        eprintln!("could not open {url}: {error}");
                        self.toolbar
                            .navigation_failed(format!("Could not open {url}: {error}"));
                    }
                }
            }
        }
    }

    /// Log what a click or key on the page led to.
    pub(crate) fn report_page_outcome(&mut self, outcome: ClickOutcome) {
        match outcome {
            ClickOutcome::Navigated(url) => {
                println!("→ {url}");
                self.scroll_y = 0.0;
            }
            ClickOutcome::NavigationFailed { url, error } => {
                eprintln!("could not open {url}: {error}");
            }
            ClickOutcome::Script | ClickOutcome::Ignored => {}
        }
    }

    pub(crate) fn navigation(&self) -> Navigation {
        Navigation::of(&self.browser)
    }
}

/// What closing a tab did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Closed {
    /// The tab was removed.
    Tab,
    /// It was the only tab. It is kept, and the window should close.
    LastTab,
    /// No tab has that index.
    Nothing,
}

/// What carrying out a [`TabCommand`] means for the window.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Applied {
    Continue,
    /// Something went wrong that the user should see (in the title bar).
    Notice(String),
    /// The last tab was closed: close the window too.
    CloseWindow,
}

/// The window's tabs, in strip order, with exactly one active.
pub(crate) struct Tabs {
    tabs: Vec<Tab>,
    active: usize,
    /// Window width, for each tab's toolbar.
    width: usize,
}

impl Tabs {
    pub(crate) fn new(first: Browser, width: usize) -> Tabs {
        Tabs {
            tabs: vec![Tab::new(first, width)],
            active: 0,
            width,
        }
    }

    pub(crate) fn len(&self) -> usize {
        self.tabs.len()
    }

    pub(crate) fn active_index(&self) -> usize {
        self.active
    }

    pub(crate) fn active(&self) -> &Tab {
        &self.tabs[self.active]
    }

    pub(crate) fn active_mut(&mut self) -> &mut Tab {
        &mut self.tabs[self.active]
    }

    #[cfg(test)]
    pub(crate) fn get(&self, index: usize) -> Option<&Tab> {
        self.tabs.get(index)
    }

    pub(crate) fn iter_mut(&mut self) -> impl Iterator<Item = &mut Tab> {
        self.tabs.iter_mut()
    }

    pub(crate) fn labels(&self) -> Vec<String> {
        self.tabs.iter().map(Tab::label).collect()
    }

    /// Add a tab showing `browser` at the end of the strip and switch to it.
    pub(crate) fn open(&mut self, browser: Browser) -> usize {
        self.tabs.push(Tab::new(browser, self.width));
        self.select(self.tabs.len() - 1);
        self.active
    }

    /// Carry out a tab-strip click or tab shortcut.
    ///
    /// A new tab is refused beyond `capacity` (what the strip can show), and
    /// `open_tab` is only called when one will be added. A new tab that fails
    /// to load is not added; the current tab stays active.
    pub(crate) fn apply(
        &mut self,
        command: TabCommand,
        capacity: usize,
        open_tab: impl FnOnce() -> Result<Browser, LoadError>,
    ) -> Applied {
        match command {
            TabCommand::New if self.tabs.len() >= capacity => {
                return Applied::Notice(format!(
                    "Tab limit reached: {capacity} tabs fit in this window"
                ));
            }
            TabCommand::New => match open_tab() {
                Ok(browser) => {
                    self.open(browser);
                    // Like other browsers, a new tab is ready for an address:
                    // its URL starts selected, so typing replaces it.
                    let tab = self.active_mut();
                    let current = tab.browser.url().to_string();
                    tab.toolbar.focus_address(&current);
                }
                Err(error) => {
                    eprintln!("could not open a new tab: {error}");
                    return Applied::Notice(format!("Could not open a new tab: {error}"));
                }
            },
            TabCommand::Close(index) => {
                if self.close(index) == Closed::LastTab {
                    return Applied::CloseWindow;
                }
            }
            TabCommand::Select(index) => {
                self.select(index);
            }
            TabCommand::Next => {
                self.select_next();
            }
            TabCommand::Previous => {
                self.select_previous();
            }
            TabCommand::Number(n) => {
                self.select_number(n);
            }
        }
        Applied::Continue
    }

    /// Switch to the tab at `index`. Returns false if that is not a tab or
    /// already the active one.
    pub(crate) fn select(&mut self, index: usize) -> bool {
        if index >= self.tabs.len() || index == self.active {
            return false;
        }
        // A gesture that started in the old tab's toolbar must not finish
        // in it while hidden. Its typed address text is kept.
        self.tabs[self.active].toolbar.reset_pointer();
        self.active = index;
        true
    }

    pub(crate) fn select_next(&mut self) -> bool {
        self.select((self.active + 1) % self.tabs.len())
    }

    pub(crate) fn select_previous(&mut self) -> bool {
        self.select((self.active + self.tabs.len() - 1) % self.tabs.len())
    }

    /// Ctrl+`n`: tabs 1–8 by position, 9 for the last tab. A number past the
    /// end does nothing, as in other browsers.
    pub(crate) fn select_number(&mut self, n: usize) -> bool {
        match n {
            9 => self.select(self.tabs.len() - 1),
            1..=8 => self.select(n - 1),
            _ => false,
        }
    }

    /// Close the tab at `index`. Closing the active tab activates the one
    /// that slides into its place (its right neighbour), or the new last
    /// tab when it was the last. The only tab is never removed: the caller
    /// closes the window instead, so there is always a tab to show.
    pub(crate) fn close(&mut self, index: usize) -> Closed {
        if index >= self.tabs.len() {
            return Closed::Nothing;
        }
        if self.tabs.len() == 1 {
            return Closed::LastTab;
        }
        // The removed tab's Browser is dropped here, cancelling its timers,
        // pending fetches and animation callbacks with its document.
        self.tabs.remove(index);
        if index < self.active || self.active == self.tabs.len() {
            self.active -= 1;
        }
        Closed::Tab
    }

    /// Give back the active tab's session, consuming the rest.
    pub(crate) fn into_active(mut self) -> Browser {
        self.tabs.swap_remove(self.active).browser
    }
}

/// The physical keys used by tab shortcuts. Text is never mapped back to
/// these names: e.g. a layout can produce `1` from a different physical key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ShortcutKey {
    Tab,
    T,
    W,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
}

/// A key used with Ctrl (without Alt/AltGr) belongs to chrome until its
/// physical release. The callback processes characters before that release,
/// so queued text keeps its ownership decision even if drained much later.
#[derive(Debug, Default)]
pub(crate) struct ShortcutKeyFilter {
    owned: std::collections::BTreeMap<u32, ShortcutKey>,
}

impl ShortcutKeyFilter {
    #[cfg(test)]
    pub(crate) fn key_state(&mut self, key: ShortcutKey, down: bool, ctrl: bool, alt: bool) {
        self.native_key_state(key as u32, Some(key), down, ctrl, alt);
    }

    pub(crate) fn native_key_state(
        &mut self,
        code: u32,
        key: Option<ShortcutKey>,
        down: bool,
        ctrl: bool,
        alt: bool,
    ) {
        if !down {
            self.owned.remove(&code);
        } else if ctrl && !alt {
            if let Some(key) = key {
                self.owned.insert(code, key);
            }
        }
    }

    pub(crate) fn owns_physical(&self, code: u32) -> bool {
        self.owned.contains_key(&code)
    }

    pub(crate) fn owns(&self, key: ShortcutKey) -> bool {
        self.owned.values().any(|owned| *owned == key)
    }
}

// ── Unit tests ────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use browser_engine::{
        input::{Key, Modifiers},
        net::{MemoryLoader, Url},
        script::dom_api,
    };

    const WIDTH: usize = 640;

    fn site() -> MemoryLoader {
        let mut site = MemoryLoader::new();
        site.insert("demo:///a.html", "<title>Page A</title><p>a</p>");
        site.insert("demo:///b.html", "<title>Page B</title><p>b</p>");
        site.insert("demo:///c.html", "<p>c</p>");
        site.insert(
            "demo:///tall.html",
            r#"<title>Tall</title><div style="height: 3000px">tall</div>"#,
        );
        site.insert(
            "demo:///keys.html",
            r#"<html><head><title>Keys</title></head><body><p id="log">none</p>
               <script>
                 let keys = "";
                 document.body.addEventListener("keydown", function (event) {
                   keys = keys + event.key;
                   document.getElementById("log").textContent = "keys " + keys;
                 });
               </script></body></html>"#,
        );
        site
    }

    fn tab(url: &str) -> Browser {
        Browser::open(Box::new(site()), &Url::parse(url).unwrap()).unwrap()
    }

    fn tabs(urls: &[&str]) -> Tabs {
        let mut tabs = Tabs::new(tab(urls[0]), WIDTH);
        for url in &urls[1..] {
            tabs.open(tab(url));
        }
        tabs
    }

    fn url(tabs: &Tabs, index: usize) -> String {
        tabs.get(index).unwrap().browser.url().to_string()
    }

    fn active_url(tabs: &Tabs) -> String {
        tabs.active().browser.url().to_string()
    }

    fn key(key: Key) -> KeyEvent {
        KeyEvent::with_modifiers(key, Modifiers::NONE)
    }

    fn chars(text: &str) -> Vec<KeyEvent> {
        text.chars().map(|c| key(Key::Character(c))).collect()
    }

    fn page_log(tab: &Tab) -> String {
        let html = dom_api::outer_html(&tab.browser.document().dom);
        let start = html.find("<p id=\"log\">").expect("log") + "<p id=\"log\">".len();
        html[start..start + html[start..].find('<').unwrap()].to_string()
    }

    // ── Creating and selecting ────────────────────────────────────────────

    #[test]
    fn a_new_tab_is_appended_and_becomes_active() {
        let mut tabs = tabs(&["demo:///a.html"]);
        assert_eq!((tabs.len(), tabs.active_index()), (1, 0));
        assert_eq!(tabs.open(tab("demo:///b.html")), 1);
        assert_eq!(active_url(&tabs), "demo:///b.html");
        tabs.select(0);
        assert_eq!(tabs.open(tab("demo:///c.html")), 2, "always at the end");
        assert_eq!(tabs.labels(), ["Page A", "Page B", "demo:///c.html"]);
    }

    #[test]
    fn next_and_previous_wrap_around() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html", "demo:///c.html"]);
        assert_eq!(tabs.active_index(), 2);
        assert!(tabs.select_next());
        assert_eq!(tabs.active_index(), 0);
        assert!(tabs.select_previous());
        assert_eq!(tabs.active_index(), 2);
        assert!(tabs.select_previous());
        assert_eq!(tabs.active_index(), 1);

        let mut single = tabs_single();
        assert!(!single.select_next(), "one tab: nothing to switch to");
        assert!(!single.select_previous());
    }

    fn tabs_single() -> Tabs {
        tabs(&["demo:///a.html"])
    }

    #[test]
    fn selecting_by_index_and_by_number() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html", "demo:///c.html"]);
        assert!(tabs.select(0));
        assert!(!tabs.select(0), "already active");
        assert!(!tabs.select(3), "no such tab");
        assert_eq!(tabs.active_index(), 0);
        assert!(tabs.select_number(2));
        assert_eq!(tabs.active_index(), 1);
        assert!(tabs.select_number(9), "9 is always the last tab");
        assert_eq!(tabs.active_index(), 2);
        assert!(!tabs.select_number(5), "past the end does nothing");
        assert!(!tabs.select_number(0));
        assert_eq!(tabs.active_index(), 2);
    }

    // ── Closing ───────────────────────────────────────────────────────────

    #[test]
    fn closing_the_active_tab_activates_its_right_neighbour_or_the_new_last() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html", "demo:///c.html"]);
        tabs.select(1);
        assert_eq!(tabs.close(1), Closed::Tab);
        assert_eq!(tabs.len(), 2);
        assert_eq!(active_url(&tabs), "demo:///c.html");
        assert_eq!(tabs.close(1), Closed::Tab);
        assert_eq!(active_url(&tabs), "demo:///a.html");
    }

    #[test]
    fn closing_another_tab_keeps_the_same_tab_active() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html", "demo:///c.html"]);
        tabs.select(2);
        assert_eq!(tabs.close(0), Closed::Tab);
        assert_eq!(active_url(&tabs), "demo:///c.html");
        assert_eq!(tabs.active_index(), 1);
        tabs.select(0);
        assert_eq!(tabs.close(1), Closed::Tab);
        assert_eq!(active_url(&tabs), "demo:///b.html");
        assert_eq!(tabs.close(7), Closed::Nothing);
    }

    #[test]
    fn closing_the_last_tab_keeps_it_and_reports_it() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html"]);
        tabs.active_mut()
            .browser
            .navigate(&Url::parse("demo:///c.html").unwrap())
            .unwrap();
        assert_eq!(tabs.close(0), Closed::Tab);
        assert_eq!(tabs.close(0), Closed::LastTab);
        assert_eq!(tabs.len(), 1);
        // The window hands the survivor back for the CLI's post-window output.
        assert_eq!(tabs.into_active().url().to_string(), "demo:///c.html");
    }

    // ── Independent state ─────────────────────────────────────────────────

    #[test]
    fn each_tab_keeps_its_own_history() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html"]);
        tabs.select(0);
        tabs.active_mut()
            .run_command(Command::Navigate("demo:///c.html".into()));
        assert_eq!(active_url(&tabs), "demo:///c.html");
        assert!(tabs.active().navigation().can_go_back);

        tabs.select(1);
        assert_eq!(active_url(&tabs), "demo:///b.html");
        let nav = tabs.active().navigation();
        assert!(
            !nav.can_go_back && !nav.can_go_forward,
            "untouched by tab 0"
        );
        assert_eq!(tabs.active().browser.history().len(), 1);

        // Back in tab 1 has nothing to do and leaves tab 0 alone.
        tabs.active_mut().run_command(Command::Back);
        assert_eq!(active_url(&tabs), "demo:///b.html");
        assert_eq!(url(&tabs, 0), "demo:///c.html");

        tabs.select(0);
        tabs.active_mut().run_command(Command::Back);
        assert_eq!(active_url(&tabs), "demo:///a.html");
        assert!(tabs.active().navigation().can_go_forward);
        assert_eq!(url(&tabs, 1), "demo:///b.html");
    }

    #[test]
    fn scroll_and_unsent_address_text_survive_switching_away_and_back() {
        let mut tabs = tabs(&["demo:///tall.html", "demo:///a.html"]);
        tabs.select(0);
        tabs.active_mut().scroll_y = 420.0;
        let current = tabs.active().browser.url().to_string();
        tabs.active_mut().toolbar.focus_address(&current);
        tabs.active_mut().handle_keys(chars("half-typed"), false);

        tabs.select(1);
        assert_eq!(tabs.active().scroll_y, 0.0);
        assert!(!tabs.active().toolbar.is_editing(), "focus is per tab");
        assert_eq!(tabs.active_mut().handle_keys(chars("x"), false), None);

        tabs.select(0);
        assert_eq!(tabs.active().scroll_y, 420.0);
        let editor = tabs.active().toolbar.editor().expect("still editing");
        assert_eq!(editor.value(), "half-typed");
        assert_eq!(active_url(&tabs), "demo:///tall.html", "nothing navigated");
    }

    #[test]
    fn keys_reach_only_the_active_tabs_page() {
        let mut tabs = tabs(&["demo:///keys.html", "demo:///keys.html"]);
        tabs.select(0);
        tabs.active_mut().handle_keys(chars("ab"), false);
        tabs.select(1);
        tabs.active_mut().handle_keys(chars("z"), false);

        assert_eq!(page_log(tabs.get(0).unwrap()), "keys ab");
        assert_eq!(page_log(tabs.get(1).unwrap()), "keys z");

        // While tab 1's address field is focused, its page gets nothing and
        // neither does tab 0's.
        tabs.active_mut().toolbar.focus_address("demo:///keys.html");
        tabs.active_mut().handle_keys(chars("qq"), false);
        assert_eq!(page_log(tabs.get(1).unwrap()), "keys z");
        assert_eq!(page_log(tabs.get(0).unwrap()), "keys ab");
        // Alt shortcuts are chrome-only too.
        tabs.select(0);
        tabs.active_mut().handle_keys(chars("r"), true);
        assert_eq!(page_log(tabs.get(0).unwrap()), "keys ab");
    }

    #[test]
    fn toolbar_commands_act_only_on_the_active_tab() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html"]);
        for tab in tabs.iter_mut() {
            tab.run_command(Command::Navigate("demo:///c.html".into()));
        }
        tabs.select(0);
        tabs.active_mut().run_command(Command::Back);
        assert_eq!(url(&tabs, 0), "demo:///a.html");
        assert_eq!(url(&tabs, 1), "demo:///c.html");
        assert!(tabs.get(1).unwrap().navigation().can_go_back);

        tabs.active_mut().run_command(Command::Forward);
        tabs.active_mut().run_command(Command::Reload);
        assert_eq!(url(&tabs, 0), "demo:///c.html");
        assert_eq!(tabs.get(0).unwrap().browser.history().len(), 2);
        assert_eq!(tabs.get(1).unwrap().browser.history().len(), 2);
    }

    #[test]
    fn a_failed_navigation_keeps_that_tabs_page_text_and_history() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html"]);
        tabs.active_mut().scroll_y = 0.0;
        tabs.active_mut().toolbar.focus_address("demo:///b.html");
        let mut typed = chars("demo:///missing.html");
        typed.push(key(Key::Enter));
        let command = tabs.active_mut().handle_keys(typed, false);
        tabs.active_mut()
            .run_command(command.expect("Enter navigates"));

        let failed = tabs.active();
        assert_eq!(failed.browser.url().to_string(), "demo:///b.html");
        assert_eq!(failed.browser.history().len(), 1);
        assert_eq!(failed.label(), "Page B", "previous document still shown");
        let editor = failed.toolbar.editor().expect("field stays open");
        assert_eq!(editor.value(), "demo:///missing.html");
        assert!(failed.toolbar.error().is_some());
        assert!(failed
            .status()
            .starts_with("Could not open demo:///missing.html"));

        // The other tab is unaffected, and the error is still there on return.
        tabs.select(0);
        assert!(tabs.active().toolbar.error().is_none());
        assert!(!tabs.active().toolbar.is_editing());
        assert_eq!(active_url(&tabs), "demo:///a.html");
        tabs.select(1);
        assert!(tabs.active().toolbar.error().is_some());
    }

    #[test]
    fn switching_tabs_cancels_a_half_finished_toolbar_click() {
        use crate::toolbar::Control;
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html"]);
        tabs.active_mut()
            .run_command(Command::Navigate("demo:///c.html".into()));
        let back = tabs.active().toolbar.rect(Control::Back);
        let (x, y) = (back.x + 4.0, back.y + 4.0);
        tabs.active_mut()
            .toolbar
            .press(x, y, "demo:///c.html", false);
        tabs.select(0);
        tabs.select(1);
        let nav = tabs.active().navigation();
        assert_eq!(tabs.active_mut().toolbar.release(x, y, nav), None);
        assert_eq!(active_url(&tabs), "demo:///c.html");
    }

    #[test]
    fn page_navigation_resets_only_that_tabs_scroll() {
        let mut tabs = tabs(&["demo:///tall.html", "demo:///tall.html"]);
        tabs.select(0);
        tabs.active_mut().scroll_y = 300.0;
        tabs.select(1);
        tabs.active_mut().scroll_y = 700.0;
        tabs.active_mut()
            .report_page_outcome(ClickOutcome::Navigated(
                Url::parse("demo:///a.html").unwrap(),
            ));
        assert_eq!(tabs.active().scroll_y, 0.0);
        assert_eq!(tabs.get(0).unwrap().scroll_y, 300.0);
    }

    // ── Applying commands ─────────────────────────────────────────────────

    fn unused() -> Result<Browser, LoadError> {
        panic!("no tab should be opened")
    }

    #[test]
    fn a_new_tab_opens_the_home_page_with_its_address_ready_to_replace() {
        let mut tabs = tabs(&["demo:///a.html"]);
        tabs.active_mut()
            .run_command(Command::Navigate("demo:///b.html".into()));
        let applied = tabs.apply(TabCommand::New, 8, || Ok(tab("demo:///a.html")));
        assert_eq!(applied, Applied::Continue);
        assert_eq!((tabs.len(), tabs.active_index()), (2, 1));
        let fresh = tabs.active();
        assert_eq!(fresh.browser.history().len(), 1, "a fresh session");
        let editor = fresh.toolbar.editor().expect("address focused");
        assert_eq!(editor.value(), "demo:///a.html");
        assert_eq!(editor.selection(), Some((0, 14)));
        // Typing goes to the new tab's field, not to any page.
        let mut typed = chars("demo:///c.html");
        typed.push(key(Key::Enter));
        let command = tabs.active_mut().handle_keys(typed, false).unwrap();
        tabs.active_mut().run_command(command);
        assert_eq!(active_url(&tabs), "demo:///c.html");
        assert_eq!(url(&tabs, 0), "demo:///b.html");
    }

    #[test]
    fn new_tabs_stop_at_capacity_and_a_failed_load_adds_nothing() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html"]);
        let applied = tabs.apply(TabCommand::New, 2, unused);
        assert!(matches!(applied, Applied::Notice(ref m) if m.contains("Tab limit")));
        assert_eq!(tabs.len(), 2);

        tabs.select(0);
        let applied = tabs.apply(TabCommand::New, 8, || {
            Err(LoadError::InvalidUrl("unreachable home".into()))
        });
        assert!(matches!(applied, Applied::Notice(ref m) if m.contains("unreachable home")));
        assert_eq!((tabs.len(), tabs.active_index()), (2, 0));
    }

    #[test]
    fn applying_select_close_and_cycle_commands() {
        let mut tabs = tabs(&["demo:///a.html", "demo:///b.html", "demo:///c.html"]);
        let apply = |tabs: &mut Tabs, command| tabs.apply(command, 8, unused);
        assert_eq!(apply(&mut tabs, TabCommand::Select(0)), Applied::Continue);
        assert_eq!(tabs.active_index(), 0);
        apply(&mut tabs, TabCommand::Previous);
        assert_eq!(tabs.active_index(), 2);
        apply(&mut tabs, TabCommand::Next);
        assert_eq!(tabs.active_index(), 0);
        apply(&mut tabs, TabCommand::Number(2));
        assert_eq!(tabs.active_index(), 1);
        apply(&mut tabs, TabCommand::Close(1));
        assert_eq!(active_url(&tabs), "demo:///c.html");
        apply(&mut tabs, TabCommand::Close(0));
        assert_eq!(apply(&mut tabs, TabCommand::Close(0)), Applied::CloseWindow);
        assert_eq!(active_url(&tabs), "demo:///c.html");
    }

    // ── Shortcut keys ─────────────────────────────────────────────────────

    #[test]
    fn shortcut_ownership_is_per_physical_key_and_lasts_until_release() {
        let mut filter = ShortcutKeyFilter::default();
        filter.key_state(ShortcutKey::Tab, true, true, false);
        for _ in 0..4 {
            // Repeats after Ctrl and Shift are up do not relinquish Tab.
            filter.key_state(ShortcutKey::Tab, true, false, false);
            assert!(filter.owns(ShortcutKey::Tab));
            assert!(!filter.owns(ShortcutKey::W));
            assert!(!filter.owns(ShortcutKey::Key1));
        }
        filter.key_state(ShortcutKey::Tab, false, false, false);
        filter.key_state(ShortcutKey::Tab, true, false, false);
        assert!(!filter.owns(ShortcutKey::Tab), "a new ordinary press");
    }

    #[test]
    fn shortcut_keys_have_independent_ownership_and_altgr_stays_text() {
        let keys = [
            ShortcutKey::Tab,
            ShortcutKey::T,
            ShortcutKey::W,
            ShortcutKey::Key1,
            ShortcutKey::Key2,
            ShortcutKey::Key3,
            ShortcutKey::Key4,
            ShortcutKey::Key5,
            ShortcutKey::Key6,
            ShortcutKey::Key7,
            ShortcutKey::Key8,
            ShortcutKey::Key9,
        ];
        let mut filter = ShortcutKeyFilter::default();
        for key in keys {
            filter.key_state(key, true, true, true);
            assert!(!filter.owns(key), "AltGr: {key:?}");
            filter.key_state(key, true, true, false);
        }
        for key in keys {
            assert!(filter.owns(key));
            filter.key_state(key, false, false, false);
            assert!(!filter.owns(key));
        }
    }
}
