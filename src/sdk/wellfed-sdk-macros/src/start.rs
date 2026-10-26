//! `#[start]`: the firmware's start hook, `wellfed_firmware_start`.
//!
//! The runtime calls it once, from its startup task, after the startup
//! checks passed and before its service task starts. The hook returns
//! whether startup work still holds the KM0 flash handshake, so a firmware
//! cannot forget to say; and it names the product runtime crate, so the
//! record stores the runtime calls cannot be left unlinked.

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{ItemFn, Path, ReturnType, Type};

pub(crate) fn expand(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mut runtime: Option<Path> = None;
    let parser = syn::meta::parser(|meta| {
        if meta.path.is_ident("runtime") {
            runtime = Some(meta.value()?.parse()?);
            Ok(())
        } else {
            Err(meta.error("expected `runtime = <crate>`"))
        }
    });
    syn::parse::Parser::parse2(parser, args)?;
    let item: ItemFn = syn::parse2(item)?;

    let runtime = runtime.ok_or_else(|| {
        syn::Error::new(
            item.sig.span(),
            "name the product runtime the firmware links \
             (`runtime = wellfed_feeder_runtime`): the SDK's settings writer and Wi-Fi join \
             call its record stores",
        )
    })?;
    check_signature(&item)?;

    let ident = &item.sig.ident;
    Ok(quote! {
        #item

        #[cfg(target_os = "none")]
        #[unsafe(no_mangle)]
        extern "C" fn wellfed_firmware_start() {
            use #runtime as _;
            ::wellfed_sdk::__started(#ident());
        }
    })
}

fn check_signature(item: &ItemFn) -> syn::Result<()> {
    let sig = &item.sig;
    if !sig.inputs.is_empty() || !sig.generics.params.is_empty() {
        return Err(syn::Error::new(
            sig.span(),
            "the start hook takes no arguments and is not generic",
        ));
    }
    if sig.asyncness.is_some() || sig.unsafety.is_some() {
        return Err(syn::Error::new(
            sig.span(),
            "the start hook is a plain safe function: it creates tasks and returns",
        ));
    }
    let returns_startup = matches!(
        &sig.output,
        ReturnType::Type(_, ty) if matches!(
            &**ty,
            Type::Path(p) if p.path.segments.last().is_some_and(|s| s.ident == "Startup")
        )
    );
    if !returns_startup {
        return Err(syn::Error::new(
            sig.output.span(),
            "the start hook returns `wellfed_sdk::Startup`: `Done`, or `Pending` while \
             startup work holds the KM0 flash handshake",
        ));
    }
    Ok(())
}
