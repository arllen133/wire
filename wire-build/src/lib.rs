use heck::ToSnakeCase;
use std::{cell::RefCell, collections::HashMap, env, ffi::OsStr, fs, path::PathBuf, vec};

use proc_macro2::TokenStream;
use quote::quote;
use syn::{
    parenthesized, parse_str, token, Attribute, Item, ItemImpl, ItemMod, ItemStruct, ItemUse, Path,
    Type, UseTree,
};

pub fn configure() -> Builder {
    Builder {
        out_dir: None,
        out_file: None,
        dir: None,
        dep: build_ident("dep"),
        variants: RefCell::new(HashMap::new()),
        injectors: Vec::new(),
        providers: HashMap::new(),
        implements: HashMap::new(),
        dependencies: RefCell::new(Vec::new()),
    }
}

pub struct Builder {
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) out_file: Option<String>,
    pub(crate) dir: Option<String>,
    dep: proc_macro2::Ident,
    variants: RefCell<HashMap<String, Variant>>,
    injectors: Vec<Provider>,
    providers: HashMap<String, Provider>,
    implements: HashMap<String, Vec<String>>,
    dependencies: RefCell<Vec<Dep>>,
}

impl Builder {
    pub fn out_dir(mut self, out_dir: String) -> Self {
        self.out_dir = Some(PathBuf::from(out_dir));

        self
    }
    pub fn out_file(mut self, out_file: String) -> Self {
        self.out_file = Some(out_file);

        self
    }

    pub fn build(mut self) {
        self.setup();
        let modules = walk_dir(self.dir.as_ref().unwrap());
        self.merge(modules);
        let mut expanded = quote! {};
        expanded.extend(self.generate_config());
        expanded.extend(self.generate());
        self.write(expanded);
    }

    fn setup(&mut self) {
        if self.out_dir.is_none() {
            self.out_dir = Some(PathBuf::from(env::var("OUT_DIR").unwrap()));
        }
        if self.out_file.is_none() {
            self.out_file = Some("wire.rs".to_string())
        }
        if self.dir.is_none() {
            self.dir = Some("src".to_string())
        }
    }

    fn merge(&mut self, modules: Vec<ModuleContext>) {
        for module in modules {
            let mut module_injectors = module.injectors;
            self.injectors.append(&mut module_injectors);

            for (k, v) in module.providers {
                self.providers.insert(k, v);
            }

            for (k, mut v) in module.implements {
                if let Some(struct_types) = self.implements.get_mut(&k) {
                    struct_types.append(&mut v);
                } else {
                    self.implements.insert(k, v);
                };
            }
        }
    }

    fn format<T: AsRef<OsStr>>(&self, command: T) {
        // rustfmt format code
        let status = std::process::Command::new("rustfmt")
            .arg(command)
            .status()
            .expect("Failed to run rustfmt");

        if !status.success() {
            panic!("Failed to format generated code with rustfmt");
        }
    }

    fn write(&self, token: TokenStream) {
        let out_dir = self.out_dir.as_ref().unwrap();
        let di = out_dir.join(self.out_file.as_ref().unwrap());
        fs::write(&di, token.to_string()).unwrap();
        self.format(&di);
    }

    fn generate_config(&self) -> TokenStream {
        let fields: Vec<_> = self
            .providers
            .values()
            .filter_map(|provider| {
                if let Some(name) = provider.metadata.config.as_ref() {
                    let field_name = build_ident(name.as_str());
                    let field_type: syn::Path = syn::parse_str(&provider.struct_type).unwrap();
                    Some(quote! {
                        pub #field_name: #field_type
                    })
                } else {
                    None
                }
            })
            .collect();

        quote! {
            #[derive(Debug, Clone, Default)]
            pub struct Config {
                #(#fields),*
            }
        }
    }

    fn generate_dependencies(&self) -> TokenStream {
        let mut dedup = HashMap::new();
        let deps: Vec<_> = self
            .dependencies
            .borrow()
            .iter()
            .filter_map(|dep| {
                let field_name = dep.ident.to_string();
                if dedup.contains_key(&field_name) {
                    None
                } else {
                    dedup.insert(field_name, true);
                    Some(dep.build_field())
                }
            })
            .collect();

        quote! {
            pub struct Dependency {
                pub config: Config,

                #(#deps),*
            }
        }
    }

    fn generate(&mut self) -> TokenStream {
        let injectors: Vec<_> = self
            .injectors
            .iter()
            .flat_map(|provider| {
                if self.variants.borrow().contains_key(&provider.struct_type) {
                    return None;
                }
                // check inject fields
                let (token, _) = self.build_provider(provider);
                Some(token)
            })
            .collect();

        let mut args = Vec::new();
        let mut fields = Vec::new();
        let variants = self.variants.borrow();
        variants.iter().for_each(|(k, v)| {
            if v.export {
                let ident = &v.ident;
                args.push(ident);
                let field_type: syn::Path = syn::parse_str(k).unwrap();
                fields.push(quote! {
                    pub #ident: #field_type,
                });
            }
        });

        // build dependencies
        let dependency = self.generate_dependencies();

        let dep = &self.dep;
        quote! {
            #dependency

            pub struct ServiceContext{
                #(#fields)*
            }

            impl ServiceContext{
                pub fn new(#dep: &Dependency) -> Self {
                    #(#injectors)*;

                    Self{
                        #(#args),*
                    }
                }
            }
        }
    }
    fn extract_struct_type(&self, inject: &Inject) -> Option<String> {
        if inject.trait_object {
            if let Some(struct_types) = self.implements.get(&inject.struct_type) {
                Some(struct_types.first().unwrap().clone())
            } else {
                None
            }
        } else if self.providers.contains_key(&inject.struct_type) {
            Some(inject.struct_type.clone())
        } else {
            None
        }
    }
    fn build_provider(&self, provider: &Provider) -> (TokenStream, TokenStream) {
        eprintln!("building provider: {:?}", provider);
        // create provider deps
        let mut deps: Vec<TokenStream> = Vec::new();
        let args: Vec<_> = provider
            .injects
            .iter()
            .map(|inject| {
                // check dep if provided
                let struct_type = self.extract_struct_type(&inject);
                let provided = struct_type.is_some();

                // provider not found
                if !provided && !inject.manual {
                    panic!("provider missing for '{}'", &inject.struct_type)
                }

                // provider manual inject provider
                if !provided && inject.manual {
                    let dep = inject.build_dep();
                    let param = dep.build_param(&self.dep);
                    self.dependencies.borrow_mut().push(dep);
                    return param;
                }

                // find struct define type
                let struct_type = &struct_type.unwrap();
                // build from cache
                if let Some(variant) = self.variants.borrow().get(struct_type) {
                    let ident = variant.ident.clone();
                    return quote! {#ident.clone()};
                }

                // cache missing, build from struct
                let provider = self.providers.get(struct_type).unwrap();
                let (dep, variant) = self.build_provider(provider);
                deps.push(dep);
                quote! {#variant.clone()}
            })
            .collect();

        // config provider
        if let Some(name) = provider.metadata.config.as_ref() {
            let mut parts = vec!["config"];
            parts.extend(name.split('.'));
            let ident_parts: Vec<_> = parts.into_iter().map(build_ident).collect();
            let dep = &self.dep;
            return (quote! {}, quote! {#dep.#(#ident_parts).*});
        }

        let name = if let Some(name) = provider.metadata.rename.as_ref() {
            name.clone()
        } else {
            provider.ident.to_snake_case()
        };
        let ident = build_ident(name.as_str());
        self.variants.borrow_mut().insert(
            provider.struct_type.clone(),
            Variant {
                ident: ident.clone(),
                export: provider.metadata.export,
            },
        );
        let path: syn::Path = parse_str(&provider.struct_type).expect(&format!(
            "failed parse struct type '{}' to path",
            &provider.struct_type
        ));
        eprintln!("build provider: {:?}", provider);
        let assign = if provider.metadata.export {
            quote! {
                let #ident = #path::new(#(#args),*);
            }
        } else {
            quote! {
                let #ident = std::sync::Arc::new(#path::new(#(#args),*));
            }
        };
        eprintln!("build provider '{}' success", provider.ident.to_string());
        (
            quote! {
                #(#deps)*

                #assign
            },
            quote! {#ident},
        )
    }
}

fn walk_dir(dir: &str) -> Vec<ModuleContext> {
    let mut modules: Vec<ModuleContext> = Vec::new();

    for res in std::fs::read_dir(dir).unwrap() {
        let entry = res.unwrap();
        let path = entry.path();
        if path.is_dir() {
            modules.append(walk_dir(path.to_str().unwrap()).as_mut());
        } else if path.extension().map_or(false, |ext| ext == "rs") {
            let mods = parse_file_path(path.as_path());
            let content = std::fs::read_to_string(&path)
                .expect(&format!("failed read file '{}'", path.display()));
            let ast = syn::parse_file(&content)
                .expect(&format!("failed parse file '{}'", path.display()));
            modules.append(parse_module(mods, ast.items).as_mut());
        }
    }

    modules
}

fn parse_module(mods: Vec<String>, items: Vec<syn::Item>) -> Vec<ModuleContext> {
    let mut modules = Vec::new();
    let mut module = ModuleContext::new(mods);
    for item in items {
        match item {
            Item::Mod(item_mod) => {
                modules.append(module.parse_item_mod(item_mod).as_mut());
            }
            Item::Use(item_use) => {
                module.parse_item_use(item_use);
            }
            Item::Struct(item_struct) => {
                module.parse_item_struct(item_struct);
            }
            Item::Impl(item_impl) => {
                module.parse_item_impl(item_impl);
            }
            _ => {}
        }
    }
    modules.push(module);

    modules
}

#[derive(Debug, Default)]
struct Metadata {
    config: Option<String>,
    export: bool,
    rename: Option<String>,
}

#[derive(Debug)]
struct Provider {
    struct_type: String,
    ident: String,
    metadata: Metadata,
    injects: Vec<Inject>,
}

impl Provider {
    pub(crate) fn new(struct_type: String, ident: String) -> Self {
        return Self {
            struct_type,
            ident,
            metadata: Metadata::default(),
            injects: Vec::new(),
        };
    }
    fn parse_attr(&mut self, attr: &Attribute) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("config") {
                if meta.input.peek(token::Paren) {
                    let content;
                    parenthesized!(content in meta.input);
                    let lit: syn::LitStr = content.parse().expect(&format!(
                        "failed parse attr 'config' content in provider '{}'",
                        self.struct_type
                    ));
                    self.metadata.config = Some(lit.value());
                } else {
                    self.metadata.config = Some(self.ident.to_snake_case());
                }
            }
            if meta.path.is_ident("export") {
                self.metadata.export = true;
            }
            if meta.path.is_ident("rename") {
                if meta.input.peek(token::Paren) {
                    let content;
                    parenthesized!(content in meta.input);
                    let lit: syn::LitStr = content.parse().expect(&format!(
                        "failed parse attr 'rename' content in provider '{}'",
                        self.struct_type
                    ));
                    self.metadata.rename = Some(lit.value());
                }
            }

            Ok(())
        });
    }
}

struct ModuleContext {
    mods: Vec<String>,
    uses: HashMap<String, Vec<String>>,
    providers: HashMap<String, Provider>,
    injectors: Vec<Provider>,
    implements: HashMap<String, Vec<String>>,
}

impl ModuleContext {
    fn new(mods: Vec<String>) -> ModuleContext {
        Self {
            mods,
            uses: HashMap::new(),
            providers: HashMap::new(),
            injectors: Vec::new(),
            implements: HashMap::new(),
        }
    }
    fn module_path(&self) -> String {
        return self.mods.join("::");
    }

    fn abs_struct_or_trait_type(&self, ident: String) -> String {
        format!("{}::{}", self.module_path(), ident)
    }

    fn resolve_abs_path_type(&self, trait_path: &Path) -> String {
        let segments: Vec<String> = trait_path
            .segments
            .iter()
            .map(|seg| seg.ident.to_string())
            .collect();

        let trait_type = segments.join("::");
        if is_absolute_path(&segments) {
            return trait_type;
        }

        let first_segment = segments.first().unwrap();
        let prefix = if let Some(absolute_path) = self.uses.get(first_segment) {
            // replace use alias and concat absolute type path
            Some(absolute_path[..absolute_path.len() - 1].join("::"))
        } else if segments.len() > 1 {
            // more then one segment, as abs path
            None
        } else {
            // default in current module
            Some(self.module_path())
        };
        if let Some(prefix) = prefix {
            format!("{}::{}", prefix, trait_type)
        } else {
            trait_type
        }
    }

    fn parse_inject_field_type(&self, mut inject: Inject, field_type: &Type) -> Option<Inject> {
        match field_type {
            Type::Path(type_path) => {
                // parse last segment type
                let segment = type_path.path.segments.last().unwrap();
                if segment.ident == "Arc" {
                    inject.wrapper_type = Some(self.resolve_abs_path_type(&type_path.path));
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner_type)) = args.args.first() {
                            return self.parse_inject_field_type(inject, inner_type);
                        }
                    }
                }

                inject.struct_type = self.resolve_abs_path_type(&type_path.path);
                Some(inject)
            }
            Type::TraitObject(trait_obj) => {
                // parse first TraitBound
                if let Some(syn::TypeParamBound::Trait(trait_bound)) = trait_obj.bounds.first() {
                    inject.struct_type = self.resolve_abs_path_type(&trait_bound.path);
                    inject.trait_object = true;
                    Some(inject)
                } else {
                    None
                }
            }
            _ => None,
        }
    }

    fn parse_provider(&self, item: &ItemStruct, attr: Option<syn::Attribute>) -> Provider {
        // struct define in current module
        let struct_path = self.abs_struct_or_trait_type(item.ident.to_string());

        let mut provider = Provider::new(struct_path, item.ident.to_string());

        // parse attribute
        if let Some(attr) = attr {
            provider.parse_attr(&attr);
        }

        // parse struct injector fields
        provider.injects = item
            .fields
            .iter()
            .filter_map(|field| {
                if let Some(attr) = get_attr(&field.attrs, "inject") {
                    // parse struct field type
                    // support field type:
                    // 1. Trait Object: dyn Bound, Box<dyn Trait>
                    // 2. Struct
                    let mut inject = Inject::default();
                    inject.parse_attr(&attr);
                    self.parse_inject_field_type(inject, &field.ty)
                } else {
                    None
                }
            })
            .collect();

        provider
    }

    fn parse_item_use(&mut self, item_use: ItemUse) {
        let items = parse_use_tree(&item_use.tree, Vec::new());
        for (ident, prefix) in items {
            self.uses.insert(ident, prefix);
        }
    }
    fn parse_item_mod(&mut self, item_mod: ItemMod) -> Vec<ModuleContext> {
        if item_mod.content.is_none() {
            return vec![];
        }
        let mut clone_mods: Vec<_> = self.mods.iter().cloned().collect();
        clone_mods.push(item_mod.ident.to_string());
        let (_, items) = item_mod.content.unwrap();
        parse_module(clone_mods, items)
    }

    fn parse_item_struct(&mut self, item_struct: ItemStruct) {
        if let Some(attr) = get_attr(&item_struct.attrs, "injectable") {
            self.injectors
                .push(self.parse_provider(&item_struct, Some(attr)));
        }

        if let Some(attr) = get_attr(&item_struct.attrs, "provider") {
            let provider = self.parse_provider(&item_struct, Some(attr));
            self.providers
                .insert(provider.struct_type.clone(), provider);
        }
    }
    fn parse_item_impl(&mut self, item_impl: ItemImpl) {
        if !has_attr(&item_impl.attrs, "provider") {
            return;
        }
        if let Some((_, trait_path, _)) = &item_impl.trait_ {
            let abs_trait_type = self.resolve_abs_path_type(trait_path);

            if let Type::Path(type_path) = item_impl.self_ty.as_ref() {
                let asb_struct_path = self.resolve_abs_path_type(&type_path.path);
                if let Some(structs) = self.implements.get_mut(&abs_trait_type) {
                    structs.push(asb_struct_path);
                } else {
                    self.implements
                        .insert(abs_trait_type, vec![asb_struct_path]);
                }
            }
        }
    }
}

fn parse_use_tree(tree: &UseTree, mut prefix: Vec<String>) -> Vec<(String, Vec<String>)> {
    match tree {
        UseTree::Path(path) => {
            let ident = path.ident.to_string();
            prefix.push(ident);
            parse_use_tree(&path.tree, prefix)
        }
        UseTree::Group(group) => group
            .items
            .iter()
            .flat_map(|tree| parse_use_tree(tree, prefix.clone()))
            .collect::<Vec<_>>(),
        UseTree::Name(name) => {
            let ident = name.ident.to_string();
            prefix.push(ident.clone());
            vec![(ident, prefix)]
        }
        UseTree::Rename(rename) => {
            let from = rename.ident.to_string();
            let to = rename.rename.to_string();
            prefix.push(from);
            vec![(to, prefix)]
        }

        _ => {
            vec![]
        }
    }
}

fn parse_file_path(path: &std::path::Path) -> Vec<String> {
    if !path.starts_with("src") {
        return vec![];
    }
    let path_buf = path.with_extension("");
    let stem = path_buf.to_str().unwrap();

    stem.split('/')
        .filter_map(|v| match v {
            "src" => Some("crate".to_string()),
            "mod" => None,
            "lib" => None,
            seg => Some(seg.to_string()),
        })
        .collect::<Vec<_>>()
}

fn has_attr(attrs: &[syn::Attribute], name: &str) -> bool {
    attrs.iter().any(|attr| attr.path().is_ident(name))
}

fn get_attr(attrs: &[syn::Attribute], name: &str) -> Option<syn::Attribute> {
    for attr in attrs {
        if attr.path().is_ident(name) {
            return Some(attr.clone());
        }
    }
    None
}

fn build_ident(name: &str) -> proc_macro2::Ident {
    syn::Ident::new(name, proc_macro2::Span::call_site())
}

fn is_absolute_path(segments: &Vec<String>) -> bool {
    if let Some(seg) = segments.first() {
        seg == "crate" || seg == ""
    } else {
        false
    }
}

struct Variant {
    ident: proc_macro2::Ident,
    export: bool,
}

struct Dep {
    ident: proc_macro2::Ident,
    path: syn::Path,
    trait_object: bool,
    wrapper_type: Option<String>,
}

impl Dep {
    fn build_param(&self, dep: &proc_macro2::Ident) -> TokenStream {
        let ident = &self.ident;
        quote! {#dep.#ident.clone()}
    }
    fn build_field(&self) -> TokenStream {
        let ident = &self.ident;
        let path = &self.path;
        if let Some(v) = self.wrapper_type.as_ref() {
            let wrapper_type: syn::Path = syn::parse_str(v).unwrap();
            if self.trait_object {
                quote! {pub #ident: #wrapper_type<dyn #path>}
            } else {
                quote! {pub #ident: #wrapper_type<#path>}
            }
        } else {
            quote! {pub #ident: #path}
        }
    }
}

#[derive(Debug, Default)]
struct Inject {
    trait_object: bool,
    wrapper_type: Option<String>,
    struct_type: String,
    manual: bool,
}

impl Inject {
    fn parse_attr(&mut self, attr: &syn::Attribute) {
        let _ = attr.parse_nested_meta(|meta| {
            if meta.path.is_ident("manual") {
                self.manual = true
            }

            Ok(())
        });
    }

    fn build_dep(&self) -> Dep {
        let path: syn::Path = syn::parse_str(&self.struct_type)
            .expect(&format!("parse struct type failed, {}", self.struct_type));
        let ident = build_ident(
            path.segments
                .last()
                .as_ref()
                .unwrap()
                .ident
                .to_string()
                .to_snake_case()
                .as_str(),
        );
        Dep {
            ident: ident.clone(),
            path,
            trait_object: self.trait_object,
            wrapper_type: self.wrapper_type.clone(),
        }
    }
}
