// External script, fetched via the relative URL "js/app.js".
// It runs in the same runtime as any inline script on the page, so the state
// it declares here survives every later click.

let clicks = 0;

const button = document.getElementById("counter");
const status = document.getElementById("status");
const log = document.getElementById("log");

function describe(n) {
    if (n === 0) { return "Waiting for a click…"; }
    return "The script ran " + n + (n === 1 ? " time" : " times");
}

function render() {
    button.textContent = "Clicks: " + clicks;
    status.textContent = describe(clicks);
    button.style.backgroundColor = clicks % 2 === 0 ? "#3498db" : "#27ae60";
}

button.addEventListener("click", function (event) {
    clicks++;
    render();

    const entry = document.createElement("li");
    entry.textContent = "click " + clicks + " on " + event.target.tagName.toLowerCase();
    entry.classList.add(clicks % 2 === 0 ? "even" : "odd");
    log.appendChild(entry);

    // Keep the list short so the page does not grow without bound.
    while (log.childElementCount > 4) {
        log.children[0].remove();
    }
});

// Tag the pipeline cells to show querySelectorAll + iteration working.
const cells = document.querySelectorAll(".grid .cell h3");
cells.forEach(cell => cell.setAttribute("data-scripted", "yes"));
console.log("app.js ready, tagged " + cells.length + " cells");

render();
