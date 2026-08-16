// Every button starts a request and returns immediately; the DOM is only
// written from a promise handler, which is a microtask that runs after the
// network completion task.

const status = document.getElementById("status");
const cards = document.getElementById("cards");
const note = document.getElementById("note");
const missing = document.getElementById("missing");
const posted = document.getElementById("posted");
const sentBody = document.getElementById("sent-body");
const all = document.getElementById("all");
const failure = document.getElementById("failure");

function mark(element, text, className) {
    element.textContent = text;
    element.className = "readout " + className;
}

// ── GET JSON ─────────────────────────────────────────────────────────────────

document.getElementById("load").addEventListener("click", function () {
    mark(status, "loading…", "loading");

    fetch("api/data.json")
        .then(function (response) {
            if (!response.ok) {
                // A bad status is a resolved promise, so turn it into a
                // rejection deliberately if that is what the page wants.
                throw "HTTP " + response.status;
            }
            return response.json();
        })
        .then(function (data) {
            mark(status, data.message + " · " + data.generator, "done");
            for (const item of data.items) {
                const row = document.createElement("li");
                const name = document.createElement("p");
                name.className = "name";
                name.textContent = item.name;
                const detail = document.createElement("p");
                detail.className = "detail";
                detail.textContent = item.detail;
                row.appendChild(name);
                row.appendChild(detail);
                cards.appendChild(row);
            }
        })
        .catch(function (error) {
            mark(status, "failed: " + error, "failed");
        });
});

// ── GET text ─────────────────────────────────────────────────────────────────

document.getElementById("load-text").addEventListener("click", function () {
    mark(note, "reading…", "loading");

    fetch("api/note.txt")
        .then(function (response) { return response.text(); })
        .then(function (text) { mark(note, text.trim(), "done"); })
        .catch(function (error) { mark(note, "failed: " + error, "failed"); });
});

// ── A missing resource ───────────────────────────────────────────────────────

document.getElementById("load-missing").addEventListener("click", function () {
    mark(missing, "requesting…", "loading");

    fetch("api/nowhere.json")
        .then(function (response) {
            // Resolved, not rejected — that is the whole point.
            mark(
                missing,
                "resolved with " + response.status + " " + response.statusText +
                    " (ok = " + response.ok + ")",
                "done"
            );
        })
        .catch(function (error) {
            mark(missing, "unexpectedly rejected: " + error, "failed");
        });
});

// ── POST ─────────────────────────────────────────────────────────────────────

document.getElementById("post").addEventListener("click", function () {
    mark(posted, "sending…", "loading");
    const body = JSON.stringify({ name: "toy browser", version: 1 });
    sentBody.textContent = "sent: " + body;

    fetch("api/echo.json", {
        method: "POST",
        headers: { "Content-Type": "application/json" },
        body: body
    })
        .then(function (response) {
            if (!response.ok) {
                // A static source answers a POST with 405, which is a perfectly
                // good response — so report it rather than treating it as an
                // error. Point the page at a real server and this branch stops
                // being taken.
                mark(
                    posted,
                    "backend answered " + response.status + " " + response.statusText,
                    "failed"
                );
                return null;
            }
            return response.json();
        })
        .then(function (data) {
            if (data) {
                mark(posted, "server said: " + data.note, "done");
            }
        })
        .catch(function (error) { mark(posted, "failed: " + error, "failed"); });
});

// ── Several at once ──────────────────────────────────────────────────────────

document.getElementById("load-all").addEventListener("click", function () {
    mark(all, "loading three…", "loading");

    Promise.all([
        fetch("api/data.json"),
        fetch("api/note.txt"),
        fetch("api/echo.json")
    ])
        .then(function (responses) {
            const parts = responses.map(function (response) {
                return response.url.split("/").pop() + "=" + response.status;
            });
            mark(all, parts.join("  ·  "), "done");
        })
        .catch(function (error) { mark(all, "failed: " + error, "failed"); });
});

// ── Failure ──────────────────────────────────────────────────────────────────

document.getElementById("fail").addEventListener("click", function () {
    mark(failure, "trying…", "loading");

    fetch("http://elsewhere.example/data.json")
        .then(function () { mark(failure, "unexpectedly succeeded", "failed"); })
        .catch(function (error) { mark(failure, "caught: " + error, "done"); });
});
