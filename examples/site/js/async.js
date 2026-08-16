// Everything on this page is driven by the event loop rather than by input.

const timeoutStatus = document.getElementById("timeout-status");
const counter = document.getElementById("tick-count");
const intervalStatus = document.getElementById("interval-status");
const box = document.getElementById("box");
const frameStatus = document.getElementById("frame-status");
const jobStatus = document.getElementById("job-status");
const log = document.getElementById("log");
const generated = document.getElementById("generated");

// ── A. setTimeout ────────────────────────────────────────────────────────────

setTimeout(function () {
    timeoutStatus.textContent = "done after " + Math.round(performance.now()) + "ms";
    timeoutStatus.className = "readout done";
}, 1000);

// ── B. setInterval, stopping itself ──────────────────────────────────────────

let count = 0;
const ticker = setInterval(function () {
    count = count + 1;
    counter.textContent = "" + count;
    if (count >= 5) {
        clearInterval(ticker);
        intervalStatus.textContent = "stopped at 5";
    }
}, 1000);

// ── C. requestAnimationFrame ─────────────────────────────────────────────────

let offset = 0;
let frames = 0;

function step(timestamp) {
    frames = frames + 1;
    offset = offset + 4;
    if (offset > 260) { offset = 0; }

    box.style.marginLeft = offset + "px";
    frameStatus.textContent = "frame " + frames + " at " + Math.round(timestamp) + "ms";
    requestAnimationFrame(step);
}

requestAnimationFrame(step);

// ── D. Scheduling and cancelling from a click ────────────────────────────────

let jobTimer = 0;
let jobInterval = 0;
let jobTicks = 0;

function note(message) {
    const entry = document.createElement("li");
    entry.textContent = message;
    log.appendChild(entry);
    while (log.childElementCount > 4) {
        log.children[0].remove();
    }
}

document.getElementById("start").addEventListener("click", function () {
    jobStatus.textContent = "running";
    note("started at " + Math.round(performance.now()) + "ms");

    jobTimer = setTimeout(function () {
        note("delayed job finished");
        jobStatus.textContent = "finished";
    }, 1500);

    jobInterval = setInterval(function () {
        jobTicks = jobTicks + 1;
        note("job tick " + jobTicks);
    }, 500);
});

document.getElementById("cancel").addEventListener("click", function () {
    clearTimeout(jobTimer);
    clearInterval(jobInterval);
    jobStatus.textContent = "cancelled";
    note("cancelled");
});

// ── E. DOM built by a timer ──────────────────────────────────────────────────

setTimeout(function () {
    for (let i = 1; i <= 3; i++) {
        const row = document.createElement("li");
        row.textContent = "row " + i + " added by a timer";
        generated.appendChild(row);
    }
}, 500);
