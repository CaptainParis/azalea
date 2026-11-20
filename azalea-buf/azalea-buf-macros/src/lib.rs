mod read;
mod write;

use proc_macro::TokenStream;
use quote::quote;
use syn::{DeriveInput, parse_macro_input};

/// Derive `AzBuf`.
///
/// Field attributes: `#[var]` (read and write with `AzBufVar`) and
/// `#[limit(n)]` (read with `AzBufLimited`). Type attribute: `#[recursive]`
/// for a type that can contain itself on the wire, so reading it holds an
/// `azalea_buf::limits::enter_nesting` guard and its depth is bounded.
#[proc_macro_derive(AzBuf, attributes(var, limit, recursive))]
pub fn derive_azbuf(input: TokenStream) -> TokenStream {
    let DeriveInput {
        ident,
        generics,
        data,
        attrs,
        ..
    } = parse_macro_input!(input);

    let (impl_generics, ty_generics, where_clause) = generics.split_for_impl();

    let writable = write::create_fn_azalea_write(&data);
    let recursive = attrs.iter().any(|a| a.path().is_ident("recursive"));
    let prelude = if recursive {
        quote! { let _nesting = azalea_buf::limits::enter_nesting()?; }
    } else {
        quote! {}
    };
    let readable = read::create_fn_azalea_read(&data, &prelude);
    let min_encoded_size = read::create_min_encoded_size(&data);
    quote! {
        #[automatically_derived]
        impl #impl_generics azalea_buf::AzBuf for #ident #ty_generics #where_clause {
            const MIN_ENCODED_SIZE: usize = #min_encoded_size;
            #writable
            #readable
        }
    }
    .into()
}
