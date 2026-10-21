//! gpu-raster owns §2.4 enforcement; gpu-apps/gpu-display migrate existing forbidden consumers.
use std::{collections::HashSet,fs,path::{Path,PathBuf}};
use syn::{visit::{self,Visit},Expr,ExprCall,ExprMethodCall,ItemFn,ImplItemFn,ItemImpl,UseTree};

#[derive(Default)]
struct Uploads { bgl:bool, texture:bool, permitted:bool, aliases:HashSet<String>, violations:Vec<String> }
impl Uploads {
    fn function(&mut self,name:&str,signature:&syn::Signature,body:&syn::Block){
        let old=self.permitted;
        let typed=|wanted:&str|signature.inputs.iter().any(|arg|matches!(arg,syn::FnArg::Typed(t) if matches!(&*t.ty,syn::Type::Path(p) if p.path.segments.last().is_some_and(|s|s.ident==wanted))));
        self.permitted=self.bgl && ((self.texture && name=="from_decoded" && typed("Decoded")) || (name=="fixture_upload" && typed("DeclaredFixture")));
        visit::visit_block(self,body);self.permitted=old;
    }
    fn upload(&mut self,name:&str){
        if matches!(name,"upload"|"upload_raw") && !self.permitted {self.violations.push(format!("{name}: outside Texture::from_decoded(Decoded) or fixture_upload(DeclaredFixture)"));}
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
            let name=last.ident.to_string();if self.aliases.contains(&name){self.upload("upload");}else{self.upload(&name);}
            if name.starts_with("syscall") && !self.bgl && e.args.iter().any(|a|match a {
                Expr::Lit(l)=>matches!(&l.lit,syn::Lit::Int(n) if n.base10_parse::<u64>().is_ok_and(|v|v==416)),
                Expr::Path(p)=>p.path.segments.last().is_some_and(|s|s.ident=="SYS_GPU"),_=>false,
            }) {self.violations.push("direct syscall 416 outside BGL".into());}
        }}visit::visit_expr_call(self,e);
    }
    fn visit_expr_method_call(&mut self,e:&'ast ExprMethodCall){self.upload(&e.method.to_string());visit::visit_expr_method_call(self,e);}
}
fn refuse(source:&str,bgl:bool)->Vec<String>{
    fn aliases(tree:&UseTree,out:&mut HashSet<String>){match tree{
        UseTree::Path(p)=>aliases(&p.tree,out),UseTree::Group(g)=>for t in &g.items{aliases(t,out)},
        UseTree::Rename(r) if matches!(r.ident.to_string().as_str(),"upload"|"upload_raw")=>{out.insert(r.rename.to_string());},_=>{}
    }}
    let ast=syn::parse_file(source).expect("owned Rust source must parse");let mut v=Uploads{bgl,..Uploads::default()};
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
    assert!(!refuse(planted,false).is_empty(),"gpu-raster: planted raw mutable-pixel uploader escaped");
    assert!(!refuse("use bgl::upload as concealed; fn f(x: &mut [u8]) {concealed(x);}",false).is_empty());
    assert!(!refuse("fn f() {raw::syscall3(416,0,0,0);}",false).is_empty());
    assert!(refuse("impl Texture {fn from_decoded(image: libimg::Decoded) {gpu::upload(image);}}",true).is_empty());
    assert!(!refuse("fn from_decoded(raw: &mut [u8]) {gpu::upload(raw);}",true).is_empty());
}
/// Production dependencies on the CPU reference, under any key: a direct key, a `package` rename,
/// or a dotted `[dependencies.<key>]` table, in normal, build and target-specific sections.
/// Development dependencies (host tests) may use it.
fn reference_dependencies(manifest:&str)->Vec<String>{
    let reference=|name:&str|name.trim().trim_matches('"').replace('_',"-")=="libgfx-ref";
    let mut out=Vec::new();let mut section=String::new();
    for line in manifest.lines().map(|l|l.split('#').next().unwrap_or("").trim()) {
        if line.starts_with('[') {section=line.trim_matches(|c|c=='['||c==']').trim().to_string();
            if let Some(key)=section.rsplit_once('.').map(|(_,k)|k) {if production(&section) && reference(key) {out.push(format!("[{section}]"));}}
            continue;}
        if !production(&section) || line.is_empty() {continue;}
        let Some((key,value))=line.split_once('=') else {continue;};
        let renamed=value.split(',').chain(core::iter::once(value)).any(|part|part.split_once('=').is_some_and(|(k,v)|k.trim().trim_start_matches('{').trim()=="package" && reference(v.trim().trim_end_matches('}'))));
        if reference(key) || (key.trim()=="package" && reference(value)) || renamed {out.push(format!("[{section}] {line}"));}
    }out
}
/// A normal, build or target-specific dependency section, or one of its dotted tables.
fn production(section:&str)->bool{
    let parts:Vec<_>=section.split('.').collect();
    let kind=match parts.as_slice() {["target",_,kind,..]=>*kind,[kind,..]=>*kind,[]=>""};
    matches!(kind,"dependencies"|"build-dependencies")
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
    // Installed programs and suites share one crate; its sources are checked below instead.
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
            if source.contains("libgfx_ref") {bad.push(format!("{relative}: uses libgfx_ref outside a suite"));}
            // The core's own device implementation and libbreenix's syscall wrapper are not API consumers.
            if relative.contains("_test") || relative.ends_with("/gpu_exec_probe.rs") || relative.ends_with("/gpu_fault_acceptance.rs"){continue;}
            for reason in refuse(&source,krate=="libs/bgl") {bad.push(format!("{relative}: {reason}"));}
        }
    }
    assert!(bad.is_empty(),"gpu-raster enforcement; gpu-apps/gpu-display consumers still require migration:\n{}",bad.join("\n"));
}
