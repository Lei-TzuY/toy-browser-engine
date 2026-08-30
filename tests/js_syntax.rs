use browser_engine::html::parse_html;
use browser_engine::script::execute_dom_scripts;

fn run_js(code: &str) -> Vec<String> {
    let html = format!("<script>{code}</script>");
    let mut dom = parse_html(&html);
    let runtime = execute_dom_scripts(&mut dom);
    runtime.console
}

#[test]
fn test_template_literals() {
    let console = run_js(
        r#"
        let name = "World";
        let age = 42;
        console.log(`Hello, ${name}!`);
        console.log(`Calculation: ${1 + 2 * 3}`);
        console.log(`Person: ${name}, Age: ${age}`);
        console.log(`Plain template with no vars`);
        console.log(`Escaped: \n\t\`end`);
        "#,
    );
    assert_eq!(
        console,
        vec![
            "Hello, World!",
            "Calculation: 7",
            "Person: World, Age: 42",
            "Plain template with no vars",
            "Escaped: \n\t`end",
        ]
    );
}

#[test]
fn test_nullish_coalescing() {
    let console = run_js(
        r#"
        let a = null ?? "default_a";
        let b = undefined ?? "default_b";
        let c = 0 ?? "default_c";
        let d = false ?? "default_d";
        let e = "" ?? "default_e";
        let f = "actual" ?? "default_f";
        console.log(a, b, c, d, e, f);
        "#,
    );
    assert_eq!(console, vec!["default_a default_b 0 false  actual"]);
}

#[test]
fn test_optional_chaining() {
    let console = run_js(
        r#"
        let obj = { user: { name: "Alice", getAge: () => 30 }, items: [10, 20] };
        console.log(obj?.user?.name);
        console.log(obj?.user?.getAge?.());
        console.log(obj?.missing?.name);
        console.log(obj?.missing?.());
        console.log(obj?.items?.[1]);
        console.log(obj?.missingList?.[0]);

        let nullObj = null;
        console.log(nullObj?.prop);
        console.log(nullObj?.[0]);
        console.log(nullObj?.());
        "#,
    );
    assert_eq!(
        console,
        vec![
            "Alice",
            "30",
            "undefined",
            "undefined",
            "20",
            "undefined",
            "undefined",
            "undefined",
            "undefined",
        ]
    );
}

#[test]
fn test_spread_in_arrays_and_objects() {
    let console = run_js(
        r#"
        let a = [2, 3];
        let b = [1, ...a, 4, 5];
        console.log(b.length);
        console.log(b[0], b[1], b[2], b[3], b[4]);

        let obj1 = { x: 1, y: 2 };
        let obj2 = { ...obj1, z: 3, y: 20 };
        console.log(obj2.x, obj2.y, obj2.z);
        "#,
    );
    assert_eq!(console, vec!["5", "1 2 3 4 5", "1 20 3"]);
}

#[test]
fn test_spread_in_function_calls_and_rest_params() {
    let console = run_js(
        r#"
        function sum(first, ...rest) {
            let total = first;
            for (let i = 0; i < rest.length; i++) {
                total += rest[i];
            }
            return total;
        }

        let nums = [20, 30];
        console.log(sum(10, ...nums));
        console.log(sum(5));

        let arrowSum = (...args) => {
            let s = 0;
            for (let x of args) {
                s += x;
            }
            return s;
        };
        console.log(arrowSum(1, 2, 3, 4));
        "#,
    );
    assert_eq!(console, vec!["60", "5", "10"]);
}

#[test]
fn test_destructuring_assignment() {
    let console = run_js(
        r#"
        let { name, age, missing } = { name: "Bob", age: 25 };
        console.log(name, age, missing);

        let { title: jobTitle, count: totalCount } = { title: "Engineer", count: 3 };
        console.log(jobTitle, totalCount);

        let [x, y, z] = [100, 200, 300];
        console.log(x, y, z);

        let [first, , third, ...tail] = [1, 2, 3, 4, 5, 6];
        console.log(first, third, tail.length, tail[0], tail[1], tail[2]);
        "#,
    );
    assert_eq!(
        console,
        vec![
            "Bob 25 undefined",
            "Engineer 3",
            "100 200 300",
            "1 3 3 4 5 6"
        ]
    );
}

#[test]
fn test_for_in_loop() {
    let console = run_js(
        r#"
        let person = { name: "Charlie", city: "Taipei" };
        let keys = [];
        for (let k in person) {
            keys.push(k);
        }
        console.log(keys.join(","));

        let arr = ["a", "b", "c"];
        let arrIndices = [];
        for (let idx in arr) {
            arrIndices.push(idx);
        }
        console.log(arrIndices.join(","));
        "#,
    );
    assert_eq!(console, vec!["name,city", "0,1,2"]);
}

#[test]
fn test_object_methods_entries_and_assign() {
    let console = run_js(
        r#"
        let target = { a: 1 };
        let source1 = { b: 2, c: 3 };
        let source2 = { c: 30, d: 4 };
        let res = Object.assign(target, source1, source2);
        console.log(res.a, res.b, res.c, res.d);
        console.log(target.c);

        let entries = Object.entries({ foo: "bar", baz: 42 });
        console.log(entries.length);
        console.log(entries[0][0], entries[0][1]);
        console.log(entries[1][0], entries[1][1]);
        "#,
    );
    assert_eq!(console, vec!["1 2 30 4", "30", "2", "foo bar", "baz 42"]);
}
