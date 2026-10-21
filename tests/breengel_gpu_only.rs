//! gpu-raster owns §2.4 enforcement; gpu-apps/gpu-display migrate existing forbidden consumers.
use std::{collections::HashSet,fs,path::{Path,PathBuf}};
use syn::{visit::{self,Visit},Expr,ExprCall,ExprMethodCall,ItemFn,ImplItemFn,ItemImpl,UseTree};

/// Named exceptions to §2.4's two sanctioned upload sites: a file, the one function in it
/// that may make one upload call, and the issue whose fix removes it. Ruling B24: the run
/// stage presents its CPU-drawn frame until gpu-display draws it with BreenGL.
const EXCEPTIONS: &[(&str, &str, &str)] = &[("libs/libbreenix/src/suite_stage.rs", "show", "#2221")];

#[derive(Default)]
struct Uploads { bgl:bool, texture:bool, permitted:bool, exception:Option<&'static str>, excepted:bool, used:usize, aliases:HashSet<String>, violations:Vec<String> }
impl Uploads {
    fn function(&mut self,name:&str,signature:&syn::Signature,body:&syn::Block){
        let (old,old_excepted)=(self.permitted,self.excepted);
        let typed=|wanted:&str|signature.inputs.iter().any(|arg|matches!(arg,syn::FnArg::Typed(t) if matches!(&*t.ty,syn::Type::Path(p) if p.path.segments.last().is_some_and(|s|s.ident==wanted))));
        self.permitted=self.bgl && ((self.texture && name=="from_decoded" && typed("Decoded")) || (name=="fixture_upload" && typed("DeclaredFixture")) || (name=="geometry_upload" && typed("CrossingTable")));
        self.excepted=self.exception==Some(name);
        visit::visit_block(self,body);self.permitted=old;self.excepted=old_excepted;
    }
    fn upload(&mut self,name:&str){
        if !matches!(name,"upload"|"upload_raw") || self.permitted {return;}
        if self.excepted && self.used==0 {self.used=1;return;}
        self.violations.push(format!("{name}: outside Texture::from_decoded(Decoded), fixture_upload(DeclaredFixture) or a listed exception"));

    }
}
impl<'ast> Visit<'ast> for Uploads {
    fn visit_item_impl(&mut self,item:&'ast ItemImpl){
        let old=self.texture;self.texture=matches!(&*item.self_ty,syn::Type::Path(p) if p.path.segments.last().is_some_and(|s|s.ident=="Texture"));
        visit::visit_item_impl(self,item);self.texture=old;
    }
    fn visit_item_fn(&mut self,f:&'ast ItemFn){self.function(&f.sig.ident.to_string(),&f.sig,&f.block);}
    fn visit_impl_item_fn(&mut self,f:&'ast ImplItemFn){self.function(&f.sig.ident.to_string(),&f.sig,&f.block);}
    fn visit_expr_call(&mut self,e:&'ast ExprCall){
        if let Expr::Path(p)=&*e.func {if let Some(last)=p.path.segments.last(){
            if p.path.segments.iter().any(|s| s.ident == "DeclaredFixture") && e.args.iter().any(|arg| {
                struct Leaks(bool);
                impl<'ast> Visit<'ast> for Leaks {
                    fn visit_expr_method_call(&mut self, e: &'ast ExprMethodCall) {
                        self.0 |= e.method == "leak";
                        visit::visit_expr_method_call(self, e);
                    }
                    fn visit_expr_call(&mut self, e: &'ast ExprCall) {
                        self.0 |= matches!(&*e.func, Expr::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "leak"));
                        visit::visit_expr_call(self, e);
                    }
                }
                let mut leaks = Leaks(false); leaks.visit_expr(arg); leaks.0
            }) { self.violations.push("leaked CPU data declared as a fixture".into()); }
            let name=last.ident.to_string();if self.aliases.contains(&name){self.upload("upload");}else{self.upload(&name);}
            if name.starts_with("syscall") && !self.bgl && e.args.iter().any(|a|match a {
                Expr::Lit(l)=>matches!(&l.lit,syn::Lit::Int(n) if n.base10_parse::<u64>().is_ok_and(|v|v==416)),
                Expr::Path(p)=>p.path.segments.last().is_some_and(|s|s.ident=="SYS_GPU"),_=>false,
            }) {self.violations.push("direct syscall 416 outside BGL".into());}
        }}visit::visit_expr_call(self,e);
    }
    fn visit_expr_method_call(&mut self,e:&'ast ExprMethodCall){self.upload(&e.method.to_string());visit::visit_expr_method_call(self,e);}
}
fn refuse(source:&str,bgl:bool)->Vec<String>{refuse_except(source,bgl,None)}
fn refuse_except(source:&str,bgl:bool,exception:Option<&'static str>)->Vec<String>{
    fn aliases(tree:&UseTree,out:&mut HashSet<String>){match tree{
        UseTree::Path(p)=>aliases(&p.tree,out),UseTree::Group(g)=>for t in &g.items{aliases(t,out)},
        UseTree::Rename(r) if matches!(r.ident.to_string().as_str(),"upload"|"upload_raw")=>{out.insert(r.rename.to_string());},_=>{}
    }}
    let ast=syn::parse_file(source).expect("owned Rust source must parse");let mut v=Uploads{bgl,exception,..Uploads::default()};
    struct Imports<'a>(&'a mut HashSet<String>);impl<'ast> Visit<'ast> for Imports<'_>{fn visit_item_use(&mut self,u:&'ast syn::ItemUse){aliases(&u.tree,self.0);}}
    Imports(&mut v.aliases).visit_file(&ast);v.visit_file(&ast);v.violations
}
fn files(path:&Path,out:&mut Vec<PathBuf>){
    let mut entries:Vec<_>=fs::read_dir(path).unwrap().map(|e|e.unwrap().path()).collect();entries.sort();
    for p in entries {if p.is_dir(){files(&p,out);}else if p.extension().is_some_and(|s|s=="rs"){out.push(p);}}
}
#[test]
fn planted_raw_pixel_library_and_renamed_upload_are_refused(){
    let planted=include_str!("fixtures/breengel-cpu-render/src/lib.rs");
    assert!(!refuse("fn f() { DeclaredFixture::table(rendered.leak()); }", false).is_empty());
    assert!(!refuse(planted,false).is_empty(),"gpu-raster: planted raw mutable-pixel uploader escaped");
    assert!(!refuse("use bgl::upload as concealed; fn f(x: &mut [u8]) {concealed(x);}",false).is_empty());
    assert!(!refuse("fn f() {raw::syscall3(416,0,0,0);}",false).is_empty());
    assert!(refuse("impl Texture {fn from_decoded(image: libimg::Decoded) {gpu::upload(image);}}",true).is_empty());
    assert!(!refuse("fn from_decoded(raw: &mut [u8]) {gpu::upload(raw);}",true).is_empty());
    // A listed exception admits one upload, in its own function only.
    assert!(refuse_except("impl Gpu {fn show(&mut self) {gpu::upload(f);}}",false,Some("show")).is_empty());
    assert!(!refuse_except("impl Gpu {fn show(&mut self) {gpu::upload(f);gpu::upload(g);}}",false,Some("show")).is_empty());
    assert!(!refuse_except("fn present() {gpu::upload(f);}",false,Some("show")).is_empty());
}
/// Production reference dependencies and their local crate names. Installed programs
/// share a manifest with the suites, so their source inspection uses these names too.
fn reference_names(manifest: &str) -> Vec<String> { package_names(manifest, "libgfx-ref") }
/// The local crate names under which a manifest's production dependencies name `wanted`.
fn package_names(manifest: &str, wanted: &str) -> Vec<String> {
    let doc: toml::Value = manifest.parse().expect("valid Cargo manifest");
    fn collect(doc: &toml::Value, wanted: &str, out: &mut Vec<String>) {
        for kind in ["dependencies", "build-dependencies"] {
            if let Some(deps) = doc.get(kind).and_then(toml::Value::as_table) {
                for (key, value) in deps {
                    let package = value.get("package").and_then(toml::Value::as_str).unwrap_or(key);
                    if package.replace('_', "-") == wanted { out.push(key.replace('-', "_")); }
                }
            }
        }
    }
    let mut out = Vec::new();
    collect(&doc, wanted, &mut out);
    if let Some(targets) = doc.get("target").and_then(toml::Value::as_table) {
        for target in targets.values() { collect(target, wanted, &mut out); }
    }
    out
}
fn reference_dependencies(manifest: &str) -> Vec<String> { reference_names(manifest) }
fn reference_uses(source: &str, names: &[String]) -> bool {
    struct Paths<'a> { names: &'a [String], found: bool }
    impl<'ast> Visit<'ast> for Paths<'_> {
        fn visit_ident(&mut self, ident: &'ast syn::Ident) {
            if self.names.iter().any(|name| ident == name) { self.found = true; }
        }
    }
    let mut paths = Paths { names, found: false };
    paths.visit_file(&syn::parse_file(source).expect("owned Rust source must parse"));
    paths.found
}
#[test]
fn renamed_or_dotted_reference_dependencies_are_refused(){
    assert!(!reference_dependencies("[dependencies]\nlibgfx-ref = { path = \"../libgfx-ref\" }\n").is_empty());
    assert!(!reference_dependencies("[dependencies]\ncpu = { package = \"libgfx-ref\", path = \"../libgfx-ref\" }\n").is_empty());
    assert!(!reference_dependencies("[dependencies.cpu]\npackage = \"libgfx-ref\"\npath = \"../libgfx-ref\"\n").is_empty());
    assert!(!reference_dependencies("[dependencies.libgfx_ref]\npath = \"../libgfx-ref\"\n").is_empty());
    assert!(!reference_dependencies("[target.'cfg(target_os = \"none\")'.build-dependencies]\nlibgfx-ref = \"*\"\n").is_empty());
    assert!(reference_dependencies("[dev-dependencies]\nlibgfx-ref = { path = \"../libgfx-ref\" }\n").is_empty());
    assert!(reference_dependencies("[dependencies]\nlibgfx = { path = \"../libgfx\" }\n").is_empty());
}
#[test]
fn production_dependencies_and_upload_sites_are_gpu_only(){
    let root=Path::new(env!("CARGO_MANIFEST_DIR"));let mut bad=Vec::new();
    let installed = fs::read_to_string(root.join("userspace/programs/Cargo.toml")).unwrap();
    let installed_names = reference_names(&installed);
    // The shared suite dependency is permitted only in suite source; aliases are resolved
    // from the manifest before checking each installed program.
    for krate in ["kernel","libs/breengel","libs/libbui","libs/libicon","libs/libfont","libs/bgl","libs/libgfx","libs/libbreenix"] {
        let manifest=fs::read_to_string(root.join(krate).join("Cargo.toml")).unwrap();
        for line in reference_dependencies(&manifest) {bad.push(format!("{krate}: production dependency on libgfx-ref: {line}"));}
    }
    for krate in ["libs/breengel","libs/libbui","libs/libicon","libs/libfont","libs/bgl","libs/libgfx","userspace/programs"] {
        let mut paths=Vec::new();files(&root.join(krate).join("src"),&mut paths);
        for path in paths {
            let relative=path.strip_prefix(root).unwrap().to_string_lossy();
            if relative.contains("/suite_") {continue;}
            let source=fs::read_to_string(&path).unwrap();
            // Only suite binaries' expectation code may name the CPU reference.
            if source.contains("libgfx_ref") || (krate == "userspace/programs" && reference_uses(&source, &installed_names)) {bad.push(format!("{relative}: uses libgfx_ref outside a suite"));}
            // The core's own device implementation and libbreenix's syscall wrapper are not API consumers.
            if relative.contains("_test") || relative.ends_with("/gpu_exec_probe.rs") || relative.ends_with("/gpu_fault_acceptance.rs"){continue;}
            for reason in refuse(&source,krate=="libs/bgl") {bad.push(format!("{relative}: {reason}"));}
        }
    }
    // libbreenix's suite runner, its stage included, calls BGL's upload only at a listed
    // exception; gpu.rs and its gpu/ module are the system call wrapper that defines upload
    // and audits it.
    let mut paths=Vec::new();files(&root.join("libs/libbreenix/src"),&mut paths);
    for path in paths {
        let relative=path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        if relative=="libs/libbreenix/src/gpu.rs" || relative.starts_with("libs/libbreenix/src/gpu/") {continue;}
        let exception=EXCEPTIONS.iter().find(|(file,_,_)|*file==relative).map(|(_,function,_)|*function);
        for reason in refuse_except(&fs::read_to_string(&path).unwrap(),false,exception) {bad.push(format!("{relative}: {reason}"));}
    }
    assert!(bad.is_empty(),"gpu-raster enforcement; gpu-apps/gpu-display consumers still require migration:\n{}",bad.join("\n"));
}

#[test]
fn installed_program_cannot_hide_reference_behind_a_package_alias() {
    let manifest = "[dependencies]\ncpu = { package = \"libgfx-ref\", path = \"../libgfx-ref\" }\n";
    let names = reference_names(manifest);
    assert!(reference_uses("fn main() { cpu::shapes::fill_rect(); }", &names));
    assert!(reference_uses("use cpu as raster; fn main() { raster::shapes::fill_rect(); }", &names));
    assert!(!reference_uses("fn main() { libgfx::shapes::fill_rect(); }", &names));
}

#[test]
fn crossing_geometry_upload_is_narrowly_typed_and_bgl_owned() {
    assert!(refuse("fn geometry_upload(table: libfont::crossings::CrossingTable) { gpu::upload(table.bytes()); }", true).is_empty());
    assert!(!refuse("fn geometry_upload(table: libfont::crossings::CrossingTable) { gpu::upload(table.bytes()); }", false).is_empty());
    assert!(!refuse("fn geometry_upload(pixels: &[u8]) { gpu::upload(pixels); }", true).is_empty());
    assert!(!refuse("fn other(table: libfont::crossings::CrossingTable) { gpu::upload(table.bytes()); }", true).is_empty());
}

#[test]
fn cpu_glyph_rasterization_is_reference_only() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    assert!(!root.join("libs/libfont/src/rasterizer.rs").exists(), "gpu-text: CPU rasterizer remains in libfont");
    let font = fs::read_to_string(root.join("libs/libfont/src/lib.rs")).unwrap();
    assert!(!font.contains("fn rasterize_glyph"), "gpu-text: CPU font raster entry point remains");
    let watcher = fs::read_to_string(root.join("libs/breengel/src/font.rs")).unwrap();
    assert!(watcher.contains("libgfx::ttf_font::{CachedFont, Font}"), "gpu-text: FontWatcher needs the GPU cache");
}

fn encoder_reachable(manifest: &Path, visited: &mut HashSet<PathBuf>) -> bool {
    let manifest = manifest.canonicalize().expect("local crate manifest");
    if !visited.insert(manifest.clone()) { return false; }
    let text = fs::read_to_string(&manifest).unwrap();
    let doc: toml::Value = text.parse().unwrap();
    if doc["package"]["name"].as_str() == Some("libimg-encode") { return true; }
    fn dependencies(doc: &toml::Value, base: &Path, visited: &mut HashSet<PathBuf>) -> bool {
        for kind in ["dependencies", "build-dependencies"] {
            if let Some(deps) = doc.get(kind).and_then(toml::Value::as_table) {
                for dep in deps.values() {
                    if let Some(path) = dep.get("path").and_then(toml::Value::as_str) {
                        if encoder_reachable(&base.join(path).join("Cargo.toml"), visited) { return true; }
                    }
                }
            }
        }
        false
    }
    let base = manifest.parent().unwrap();
    dependencies(&doc, base, visited) || doc.get("target").and_then(toml::Value::as_table)
        .is_some_and(|targets| targets.values().any(|target| dependencies(target, base, visited)))
}
/// The path dependencies of a manifest other than those naming `except`, by manifest path.
fn path_dependencies_except(manifest: &Path, except: &str) -> Vec<PathBuf> {
    let doc: toml::Value = fs::read_to_string(manifest).unwrap().parse().unwrap();
    let base = manifest.parent().unwrap();
    let mut out = Vec::new();
    let mut collect = |doc: &toml::Value| for kind in ["dependencies", "build-dependencies"] {
        for (key, dep) in doc.get(kind).and_then(toml::Value::as_table).into_iter().flatten() {
            let package = dep.get("package").and_then(toml::Value::as_str).unwrap_or(key);
            if package.replace('_', "-") == except { continue; }
            if let Some(path) = dep.get("path").and_then(toml::Value::as_str) { out.push(base.join(path).join("Cargo.toml")); }
        }
    };
    collect(&doc);
    for target in doc.get("target").and_then(toml::Value::as_table).into_iter().flat_map(|t| t.values()) { collect(target); }
    out
}

#[test]
fn png_encoder_is_unreachable_from_upload_capable_crates() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    let decoder = fs::read_to_string(root.join("libs/libimg/src/png/mod.rs")).unwrap();
    assert!(!decoder.contains("fn encode("), "gpu-text: encoder remains in the upload decoder crate");
    for krate in ["kernel", "libs/bgl", "libs/libgfx", "libs/libbui", "libs/libicon", "libs/breengel", "libs/libbreenix"] {
        assert!(!encoder_reachable(&root.join(krate).join("Cargo.toml"), &mut HashSet::new()),
            "{krate}: PNG encoder reachable through production dependencies");
    }
    // Ruling B26: the suites share userspace/programs' manifest with the installed programs, so
    // its direct encoder dependency is the suites' expectation allowance, as libgfx-ref's is.
    // No other dependency may reach the encoder, and no source outside a suite may name it.
    let programs = root.join("userspace/programs/Cargo.toml");
    for dependency in path_dependencies_except(&programs, "libimg-encode") {
        assert!(!encoder_reachable(&dependency, &mut HashSet::new()),
            "userspace/programs: PNG encoder reachable through {}", dependency.display());
    }
    let names = package_names(&fs::read_to_string(&programs).unwrap(), "libimg-encode");
    let mut paths = Vec::new(); files(&root.join("userspace/programs/src"), &mut paths);
    for path in paths {
        let relative = path.strip_prefix(root).unwrap().to_string_lossy().into_owned();
        if relative.contains("/suite_") { continue; }
        let source = fs::read_to_string(&path).unwrap();
        assert!(!source.contains("libimg_encode") && !reference_uses(&source, &names),
            "{relative}: names the PNG encoder outside a suite");
    }
}

#[test]
fn installed_program_cannot_hide_the_encoder_behind_a_package_alias() {
    let names = package_names("[dependencies]\nenc = { package = \"libimg-encode\", path = \"../e\" }\n", "libimg-encode");
    assert!(reference_uses("fn main() { enc::encode(&i); }", &names));
    assert!(reference_uses("use enc as quiet; fn main() { quiet::encode(&i); }", &names));
    assert!(!reference_uses("fn main() { libimg::png::decode(&b); }", &names));
}
