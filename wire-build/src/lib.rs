use heck::ToSnakeCase;
use std::{collections::HashMap, env, ffi::OsStr, fs, path::PathBuf, vec};

use proc_macro2::TokenStream;
use quote::quote;
use syn::{parse_str, Item, ItemImpl, ItemMod, ItemStruct, ItemUse, Path, Type, UseTree};

pub fn configure() -> Builder {
    Builder {
        out_dir: None,
        out_file: None,
        variants: HashMap::new(),
        injectors: Vec::new(),
        providers: HashMap::new(),
        implements: HashMap::new(),
    }
}

pub struct Builder {
    pub(crate) out_dir: Option<PathBuf>,
    pub(crate) out_file: Option<String>,
    variants: HashMap<String, proc_macro2::Ident>,
    injectors: Vec<Provider>,
    providers: HashMap<String, Provider>,
    implements: HashMap<String, Vec<String>>,
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
    pub fn parse_dir(mut self, dir: String) -> Self {
        let modules = walk_dir(&dir);
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
        self
    }

    pub fn build(mut self) {
        self.setup();
        let token = self.generate();
        self.write(token);
    }
    fn setup(&mut self) {
        if self.out_dir.is_none() {
            self.out_dir = Some(PathBuf::from(env::var("OUT_DIR").unwrap()));
        }
        if self.out_file.is_none() {
            self.out_file = Some("wire.rs".to_string())
        }
    }

    fn format<T: AsRef<OsStr>>(&self, command: T) {
        // 调用 rustfmt 格式化生成的代码
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

    fn generate(&mut self) -> TokenStream {
        let injectors: Vec<_> = self
            .injectors
            .iter()
            .flat_map(|provider| {
                if self.variants.contains_key(&provider.struct_type) {
                    return None;
                }
                // check inject fields
                let (token, _) = self.build_provider(provider);
                Some(token)
            })
            .collect();

        quote! {
            pub fn inject(){
                #(#injectors)*;
            }
        }
    }
    fn build_provider(&self, provider: &Provider) -> (TokenStream, proc_macro2::Ident) {
        // create provider deps
        let mut deps: Vec<TokenStream> = Vec::new();
        let args: Vec<_> = provider
            .inject_fields
            .iter()
            .map(|field| {
                // find struct define type
                let struct_type = if let Some(struct_types) = self.implements.get(field) {
                    struct_types.first().unwrap_or_else(|| field)
                } else {
                    field
                };

                // build from cache
                if let Some(variant) = self.variants.get(struct_type) {
                    return quote! {#variant.clone()};
                }

                // cache missing, build from struct
                let provider = self.providers.get(struct_type).unwrap();
                let (dep, variant) = self.build_provider(provider);
                deps.push(dep);
                quote! {#variant.clone()}
            })
            .collect();
        let variant = proc_macro2::Ident::new(
            &format!("{}", provider.ident.to_snake_case()),
            proc_macro2::Span::call_site(),
        );
        let path: syn::Path =
            parse_str(&provider.struct_type).expect("failed parse struct type to path");
        let assign = quote! {
            let #variant = #path::new(#(#args),*);
        };
        (
            quote! {
                #(#deps)*

                #assign
            },
            variant,
        )
    }
}

fn walk_dir(dir: &str) -> Vec<ModuleContext> {
    let mut modules: Vec<ModuleContext> = Vec::new();

    for res in std::fs::read_dir(dir).unwrap() {
        let entry = res.unwrap();
        let path = entry.path();
        if path.is_dir() {
            let mut sub_modules = walk_dir(path.to_str().unwrap());
            modules.append(&mut sub_modules);
        } else if path.extension().map_or(false, |ext| ext == "rs") {
            let mods = parse_file_path(path.as_path());
            let content = std::fs::read_to_string(&path).unwrap();
            let ast = syn::parse_file(&content).unwrap();
            let mut sub_modules = parse_module(mods, ast.items);
            modules.append(&mut sub_modules);
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
                for sub_module in module.parse_item_mod(item_mod) {
                    modules.push(sub_module);
                }
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

struct Provider {
    struct_type: String,
    ident: String,
    config: Option<String>,
    inject_fields: Vec<String>,
}
impl Provider {
    pub(crate) fn new(struct_type: String, ident: String) -> Self {
        return Self {
            struct_type,
            ident,
            config: None,
            inject_fields: Vec::new(),
        };
    }
}

struct Inject {
    name: String,
    ty: String,
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
    fn resolve_prefix_path_type(&self, trait_path: &Path) -> String {
        let segments: Vec<String> = trait_path
            .segments
            .iter()
            .map(|seg| seg.ident.to_string())
            .collect();

        if is_absolute_path(&segments) {
            return segments[..segments.len() - 1].join("::");
        }

        let first_segment = segments.first().unwrap();
        if let Some(absolute_path) = self.uses.get(first_segment) {
            // replace use alias and concat absolute type path
            absolute_path[..absolute_path.len() - 1].join("::")
        } else {
            // default in current module
            self.module_path()
        }
    }

    fn resolve_abs_path_type(&self, trait_path: &Path) -> String {
        let prefix = self.resolve_prefix_path_type(trait_path);
        let segment = trait_path.segments.last().unwrap();
        format!("{}::{}", prefix, segment.ident.to_string())
    }

    fn extract_field_path(&self, field_type: &Type) -> Option<Path> {
        match field_type {
            Type::Path(type_path) => {
                // parse last segment type
                let segment = type_path.path.segments.last().unwrap();
                if segment.ident == "Arc" || segment.ident == "Box" {
                    if let syn::PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(syn::GenericArgument::Type(inner_type)) = args.args.first() {
                            return self.extract_field_path(inner_type);
                        }
                    }
                }

                Some(type_path.path.clone())
            }
            Type::TraitObject(trait_obj) => {
                // parse first TraitBound
                if let Some(syn::TypeParamBound::Trait(trait_bound)) = trait_obj.bounds.first() {
                    return Some(trait_bound.path.clone());
                }
                return None;
            }
            _ => None,
        }
    }

    fn parse_provider(&self, item: &ItemStruct, attr: Option<syn::Attribute>) -> Provider {
        // parse attribute

        // struct define in current module
        let struct_path = self.abs_struct_or_trait_type(item.ident.to_string());

        let mut provider = Provider::new(struct_path, item.ident.to_string());
        // parse struct injector fields
        provider.inject_fields = item
            .fields
            .iter()
            .filter_map(|field| {
                if !has_attr(&field.attrs, "inject") {
                    return None;
                }

                // parse struct field type
                // support field type:
                // 1. Trait Object: dyn Bound, Box<dyn Trait>
                // 2. Struct
                let field_type_path = self.extract_field_path(&field.ty).unwrap();
                Some(self.resolve_abs_path_type(&field_type_path))
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
        if has_attr(&item_struct.attrs, "injectable") {
            self.injectors.push(self.parse_provider(&item_struct, None));
        }

        let attr = get_attr(&item_struct.attrs, "provider");
        if attr.is_some() {
            let provider = self.parse_provider(&item_struct, attr);
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
    // 1. 确保路径在 src 目录下
    if !path.starts_with("src") {
        return vec![];
    }

    // 2. 剥离 src/ 前缀和 .rs 后缀
    let path_buf = path.with_extension("");
    let stem = path_buf.to_str().unwrap();

    // 3. 处理特殊文件名 mod.rs
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

fn is_absolute_path(segments: &Vec<String>) -> bool {
    if let Some(seg) = segments.first() {
        seg == "crate"
    } else {
        false
    }
}
