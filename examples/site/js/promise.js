// Every line below records when it actually ran, so the rendered log shows
// the real task/microtask ordering rather than the source order.

const orderLog = document.getElementById("order-log");
const chain = document.getElementById("chain");
const rejection = document.getElementById("rejection");
const cleanup = document.getElementById("cleanup");
const all = document.getElementById("all");
const rows = document.getElementById("rows");

let step = 0;

function note(message) {
    step = step + 1;
    const entry = document.createElement("li");
    entry.textContent = step + ". " + message;
    orderLog.appendChild(entry);
}

// ── 1. Ordering: sync, microtask, timer ──────────────────────────────────────

note("sync start");

// A timer is a *task*: it waits behind every microtask.
setTimeout(function () {
    note("timer (task)");

    // A microtask queued from a task runs before the next task.
    queueMicrotask(function () {
        note("microtask from the timer");
    });
}, 0);

// The executor body runs synchronously, right here.
new Promise(function (resolve) {
    note("executor (synchronous)");
    resolve("ready");
}).then(function (value) {
    note("promise then: " + value);
});

queueMicrotask(function () {
    note("queueMicrotask");
});

note("sync end");

// ── 2. Chaining ──────────────────────────────────────────────────────────────

Promise.resolve(1)
    .then(function (x) { return x + 1; })
    .then(function (x) { return x + 1; })
    .then(function (x) {
        chain.textContent = "1 + 1 + 1 = " + x;
        chain.className = "readout done";
    });

// ── 3. Rejection, recovery and finally ───────────────────────────────────────

Promise.resolve()
    .then(function () {
        throw "something broke";
    })
    .catch(function (reason) {
        return "recovered from: " + reason;
    })
    .then(function (value) {
        rejection.textContent = value;
        rejection.className = "readout done";
    })
    .finally(function () {
        cleanup.textContent = "finally ran";
    });

// ── 4. Promise.all, including one backed by a timer ──────────────────────────

const delayed = new Promise(function (resolve) {
    setTimeout(function () { resolve("from a timer"); }, 600);
});

Promise.all([Promise.resolve("first"), "a plain value", delayed])
    .then(function (values) {
        all.textContent = values.join(" | ");
        all.className = "readout done";
    });

// ── 5. A chain that waits, then builds DOM ───────────────────────────────────

Promise.resolve()
    .then(function () {
        // Returning a promise makes the chain wait for it.
        return new Promise(function (resolve) {
            setTimeout(function () { resolve(3); }, 300);
        });
    })
    .then(function (count) {
        for (let i = 1; i <= count; i++) {
            const row = document.createElement("li");
            row.textContent = "row " + i + " added by a promise";
            rows.appendChild(row);
        }
        note("promise built the DOM");
    });
