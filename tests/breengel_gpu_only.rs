//! gpu-raster owns §2.4 enforcement; gpu-apps/gpu-display migrate existing forbidden consumers.
use std::{fs,path::{Path,PathBuf}};
use syn::{visit::{self,Visit},Expr,ExprCall,ExprMethodCall,ItemFn,ImplItemFn};

#[derive(Default)]
struct Uploads { bgl:bool, permitted:bool, violations:Vec<String> }
impl Uploads {
    fn function(&mut self,name:&str,signature:&syn::Signature,body:&syn::Block){
        let old=self.permitted;
        let typed=|wanted:&str|signature.inputs.iter().any(|arg|matches!(arg,syn::FnArg::Typed(t) if matches!(&*t.ty,syn::Type::Path(p) if p.path.segments.last().is_some_and(|s|s.ident==wanted))));
        self.permitted=self.bgl && ((name=="from_decoded" && typed("Decoded")) || (name=="fixture_upload" && typed("DeclaredFixture")));
        visit::visit_block(self,body);self.permitted=old;
    }
    fn upload(&mut self,name:&str){
        if matches!(name,"upload"|"upload_raw") && !self.permitted {self.violations.push(format!("{name}: outside Texture::from_decoded(Decoded) or fixture_upload(DeclaredFixture)"));}
    }
}
impl<'ast> Visit<'ast> for Uploads {
    fn visit_item_fn(&mut self,f:&'ast ItemFn){self.function(&f.sig.ident.to_string(),&f.sig,&f.block);}
    fn visit_impl_item_fn(&mut self,f:&'ast ImplItemFn){self.function(&f.sig.ident.to_string(),&f.sig,&f.block);}
    fn visit_expr_call(&mut self,e:&'ast ExprCall){
        if let Expr::Path(p)=&*e.func {if let Some(last)=p.path.segments.last(){
            let name=last.ident.to_string();self.upload(&name);
            if name.starts_with("syscall") && !self.bgl && e.args.iter().any(|a|match a {
                Expr::Lit(l)=>matches!(&l.lit,syn::Lit::Int(n) if n.base10_parse::<u64>()==Ok(416)),
                Expr::Path(p)=>p.path.segments.last().is_some_and(|s|s.ident=="SYS_GPU"),_=>false,
            }) {self.violations.push("direct syscall 416 outside BGL".into());}
        }}visit::visit_expr_call(self,e);
    }
    fn visit_expr_method_call(&mut self,e:&'ast ExprMethodCall){self.upload(&e.method.to_string());visit::visit_expr_method_call(self,e);}
}
fn refuse(source:&str,bgl:bool)->Vec<String>{
    let ast=syn::parse_file(source).expect("owned Rust source must parse");let mut v=Uploads{bgl,..Uploads::default()};v.visit_file(&ast);v.violations
}
fn files(path:&Path,out:&mut Vec<PathBuf>){
    let mut entries:Vec<_>=fs::read_dir(path).unwrap().map(|e|e.unwrap().path()).collect();entries.sort();
    for p in entries {if p.is_dir(){files(&p,out);}else if p.extension().is_some_and(|s|s=="rs"){out.push(p);}}
}
#[test]
fn planted_raw_pixel_library_and_renamed_upload_are_refused(){
    let planted=include_str!("fixtures/breengel-cpu-render/src/lib.rs");
    assert!(!refuse(planted,false).is_empty(),"gpu-raster: planted raw mutable-pixel uploader escaped");
    assert!(!refuse("use bgl::upload as concealed; fn f(x: &mut [u8]) {bgl::upload(x);}",false).is_empty());
    assert!(!refuse("fn f() {raw::syscall3(416,0,0,0);}",false).is_empty());
    assert!(refuse("fn from_decoded(image: libimg::Decoded) {gpu::upload(image);}",true).is_empty());
    assert!(!refuse("fn from_decoded(raw: &mut [u8]) {gpu::upload(raw);}",true).is_empty());
}
#[test]
fn production_dependencies_and_upload_sites_are_gpu_only(){
    let root=Path::new(env!("CARGO_MANIFEST_DIR"));let mut bad=Vec::new();
    for krate in ["kernel","libs/breengel","libs/libbui","libs/libicon","libs/libfont","libs/bgl","userspace/programs"] {
        let manifest=fs::read_to_string(root.join(krate).join("Cargo.toml")).unwrap();
        if manifest.lines().any(|l|l.trim_start().starts_with("libgfx-ref")){bad.push(format!("{krate}: production dependency on libgfx-ref"));}
        // The core's own device implementation and libbreenix's syscall wrapper are not API consumers.
        if krate=="kernel" {continue;}
        let mut paths=Vec::new();files(&root.join(krate).join("src"),&mut paths);
        for path in paths {
            let relative=path.strip_prefix(root).unwrap().to_string_lossy();
            if relative.contains("/suite_") || relative.contains("_test") || relative.ends_with("/gpu_exec_probe.rs") || relative.ends_with("/gpu_fault_acceptance.rs"){continue;}
            for reason in refuse(&fs::read_to_string(&path).unwrap(),krate=="libs/bgl") {bad.push(format!("{relative}: {reason}"));}
        }
    }
    assert!(bad.is_empty(),"gpu-raster enforcement; gpu-apps/gpu-display consumers still require migration:\n{}",bad.join("\n"));
}
