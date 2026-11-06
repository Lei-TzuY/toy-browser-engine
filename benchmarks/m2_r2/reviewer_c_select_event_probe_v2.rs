use browser_engine::input::{Key, KeyEvent};
use browser_engine::script::dom_api;
use browser_engine::select_state;
use browser_engine::{Browser, MemoryLoader};
use browser_engine::net::Url;
fn make(handlers: &str) -> Browser {
 let html=format!(r#"<div id="host"><div id="dest"></div><select id="a"><option>A</option><option>B</option></select><select id="b"><option>A</option><option>B</option></select></div><script>{}</script>"#,handlers);
 let mut loader=MemoryLoader::new();loader.insert("demo:///index",html.as_str());
 let mut b=Browser::open(Box::new(loader),&Url::parse("demo:///index").unwrap()).unwrap(); b.document_mut().runtime.quiet=true;b
}
fn path(b:&Browser,id:&str)->Vec<usize>{dom_api::get_element_by_id(&b.document().dom,id).unwrap()}
fn value(b:&Browser,id:&str)->String{select_state::value(&b.document().dom,&path(b,id)).unwrap()}
fn down(b:&mut Browser){let p=path(b,"a");b.document_mut().focus_path(&p);b.press_key(&KeyEvent::new(Key::ArrowDown));}
#[test]fn keydown_remove_never_targets_a_neighbor(){
 let mut b=make(r#"document.getElementById("a").addEventListener("keydown",function(){document.getElementById("a").remove();});
 document.getElementById("host").addEventListener("change",function(e){console.log("change:"+e.target.id);});"#);
 down(&mut b);assert_eq!(value(&b,"b"),"A");assert_eq!(b.document().runtime.console,Vec::<String>::new());
}
#[test]fn keydown_move_keeps_original_default_target(){
 let mut b=make(r#"document.getElementById("a").addEventListener("keydown",function(){document.getElementById("dest").appendChild(document.getElementById("a"));});
 document.getElementById("host").addEventListener("change",function(e){console.log("change:"+e.target.id+":"+document.getElementById("a").value);});"#);
 down(&mut b);assert_eq!(value(&b,"a"),"B");assert_eq!(value(&b,"b"),"A");assert_eq!(b.document().runtime.console,["change:a:B"]);
}
#[test]fn input_move_keeps_original_change_target(){
 let mut b=make(r#"document.getElementById("a").addEventListener("input",function(){console.log("input");document.getElementById("dest").appendChild(document.getElementById("a"));});
 document.getElementById("host").addEventListener("change",function(e){console.log("change:"+e.target.id+":"+document.getElementById("a").value);});"#);
 down(&mut b);assert_eq!(value(&b,"a"),"B");assert_eq!(value(&b,"b"),"A");assert_eq!(b.document().runtime.console,["input","change:a:B"]);
}
#[test]fn keydown_disabling_select_blocks_default(){
 let mut b=make(r#"document.getElementById("a").addEventListener("keydown",function(){document.getElementById("a").setAttribute("disabled","");});
 document.getElementById("host").addEventListener("change",function(e){console.log("change");});"#);
 down(&mut b);assert_eq!(value(&b,"a"),"A");assert_eq!(value(&b,"b"),"A");assert_eq!(b.document().runtime.console,Vec::<String>::new());
}
#[test]fn order_and_prevent_default(){
 let mut b=make(r#"document.getElementById("a").addEventListener("keydown",function(e){console.log("down:"+document.getElementById("a").value);if(e.key==="Home"){e.preventDefault();}});
 document.getElementById("a").addEventListener("input",function(){console.log("input:"+document.getElementById("a").value);});
 document.getElementById("a").addEventListener("change",function(){console.log("change:"+document.getElementById("a").value);});
 document.getElementById("a").addEventListener("keyup",function(){console.log("up:"+document.getElementById("a").value);});"#);
 down(&mut b);b.press_key(&KeyEvent::new(Key::Home));assert_eq!(value(&b,"a"),"B");
 assert_eq!(b.document().runtime.console,["down:A","input:B","change:B","up:B","down:B","up:B"]);
}
