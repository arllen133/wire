use proc_macro::TokenStream;
use quote::quote;
use syn::{parse_macro_input, ItemStruct};

#[proc_macro_attribute]
pub fn provider(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn config(_attr: TokenStream, item: TokenStream) -> TokenStream {
    item
}

#[proc_macro_attribute]
pub fn injectable(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut ast = parse_macro_input!(item as ItemStruct);
    let ident = &ast.ident;
    let (impl_generics, ty_generics, where_clause) = ast.generics.split_for_impl();
    ast.attrs.retain(|attr| !attr.path().is_ident("injectable"));

    let mut inject_params = Vec::new();
    let mut struct_fields = Vec::new();
    for field in &mut ast.fields {
        // filter `inject` attr
        let is_inject = field
            .attrs
            .iter()
            .any(|attr| attr.path().is_ident("inject"));
        let name = field.ident.as_ref().unwrap();
        let ty = &field.ty;
        if is_inject {
            // remove field attr #[inject]
            field.attrs.retain(|attr| !attr.path().is_ident("inject"));
            inject_params.push(quote! {#name: #ty});
            struct_fields.push(quote! {#name});
        } else {
            struct_fields.push(quote! {#name: #ty::default()});
        }
    }

    let expanded = quote! {
        #ast

        impl #impl_generics #ident #ty_generics #where_clause {
            pub fn new(#(#inject_params),*) -> Self {
                Self {
                    #(#struct_fields),*
                }
            }
        }
    };
    expanded.into()
}
