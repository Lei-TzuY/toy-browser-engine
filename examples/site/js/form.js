// External script for the form page: listens to the whole interaction model —
// focus, keyboard, input, change and submit — and shows what fired.

const status = document.getElementById("status");
const events = document.getElementById("events");
const query = document.getElementById("q");
const form = document.getElementById("search");

let count = 0;

function log(message) {
    count++;
    status.textContent = message;

    const entry = document.createElement("li");
    entry.textContent = count + ". " + message;
    events.appendChild(entry);

    // Keep the log short so the page does not grow without bound.
    while (events.childElementCount > 6) {
        events.children[0].remove();
    }
}

// Focus events: focusin bubbles, so one listener on the form covers every control.
form.addEventListener("focusin", function (event) {
    log("focusin on " + describe(event.target));
});
form.addEventListener("focusout", function (event) {
    log("focusout from " + describe(event.target));
});

// Typing: keydown fires first, then the default action, then input.
query.addEventListener("keydown", function (event) {
    if (event.key === "Escape") {
        // preventDefault stops the browser's own handling of this key.
        event.preventDefault();
        log("Escape cancelled");
    }
});

query.addEventListener("input", function () {
    log("input: " + query.value + " (" + query.value.length + " chars)");
});

// Checkboxes and radios report change after their default action toggles them.
form.addEventListener("change", function (event) {
    log("change: " + describe(event.target) + " = " + event.target.checked);
});

// A submit listener sees the event before the navigation happens.
form.addEventListener("submit", function () {
    log("submit: q=" + query.value);
});

// The third button cancels its own submission to show preventDefault working.
document.getElementById("quiet").addEventListener("click", function (event) {
    event.preventDefault();
    log("submit cancelled by preventDefault");
});

function describe(element) {
    const id = element.id;
    if (id) { return "#" + id; }
    return element.tagName.toLowerCase();
}

log("ready — activeElement is " + document.activeElement.tagName.toLowerCase());
