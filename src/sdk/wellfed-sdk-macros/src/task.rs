//! `#[task]`: a safe step function as a task the runtime can create.
//!
//! The entry keeps the function's name and stays non-generic, because the
//! stack audit finds it in the linked image by that name and walks its call
//! graph. The audit reads the attribute's `name`, `stack` and `priority` from
//! the source, so they keep the forms it parses: a string, a literal or
//! constant name, and a literal.

use std::ffi::CString;

use proc_macro2::TokenStream;
use quote::quote;
use syn::spanned::Spanned;
use syn::{Expr, ExprLit, ExprPath, ItemFn, Lit, LitCStr, LitInt, LitStr, ReturnType, Type};

pub(crate) fn expand(args: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let mut name: Option<LitStr> = None;
    let mut stack: Option<Expr> = None;
    let mut priority: Option<LitInt> = None;
    let mut given = 0;
    let parser = syn::meta::parser(|meta| {
        // The stack audit reads the arguments in this order.
        let expected = ["name", "stack", "priority"].get(given).copied();
        if expected.is_none_or(|key| !meta.path.is_ident(key)) {
            return Err(
                meta.error("a task's arguments are `name`, `stack`, `priority`, in that order")
            );
        }
        given += 1;
        if meta.path.is_ident("name") {
            name = Some(meta.value()?.parse()?);
        } else if meta.path.is_ident("stack") {
            stack = Some(meta.value()?.parse()?);
        } else {
            priority = Some(meta.value()?.parse()?);
        }
        Ok(())
    });
    syn::parse::Parser::parse2(parser, args)?;
    let item: ItemFn = syn::parse2(item)?;

    let span = item.sig.span();
    let name = name.ok_or_else(|| syn::Error::new(span, "a task needs `name = \"…\"`"))?;
    let stack = stack.ok_or_else(|| syn::Error::new(span, "a task needs `stack = WORDS`"))?;
    let priority = priority.ok_or_else(|| syn::Error::new(span, "a task needs `priority = N`"))?;
    let cname = task_name(&name)?;
    check_stack(&stack)?;
    check_signature(&item)?;

    let ItemFn {
        attrs,
        vis,
        sig,
        block,
    } = item;
    let ident = sig.ident;
    Ok(quote! {
        #(#attrs)*
        #[expect(non_upper_case_globals, reason = "the task keeps its function's name, which the stack audit reads")]
        #vis const #ident: ::wellfed_sdk::Task = {
            // Ignores its context, so any caller may pass any pointer.
            extern "C" fn #ident(_: *mut ::core::ffi::c_void) -> u32 #block
            ::wellfed_sdk::Task::__new(#cname, #stack, #priority, #ident)
        };
    })
}

/// The SDK's task names, as the stack audit matches them: letters, digits,
/// `_` and `-`.
fn task_name(name: &LitStr) -> syn::Result<LitCStr> {
    let value = name.value();
    let valid = !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-');
    if !valid {
        return Err(syn::Error::new(
            name.span(),
            "a task name is ASCII letters, digits, `_` and `-`",
        ));
    }
    let cstring = CString::new(value).map_err(|e| syn::Error::new(name.span(), e))?;
    Ok(LitCStr::new(&cstring, name.span()))
}

fn check_stack(stack: &Expr) -> syn::Result<()> {
    match stack {
        Expr::Lit(ExprLit {
            lit: Lit::Int(_), ..
        }) => Ok(()),
        Expr::Path(ExprPath {
            path, qself: None, ..
        }) if path.get_ident().is_some() => Ok(()),
        _ => Err(syn::Error::new(
            stack.span(),
            "the stack is a literal or a `const NAME: u16` in the firmware's sources, \
             so the stack audit can read it",
        )),
    }
}

fn check_signature(item: &ItemFn) -> syn::Result<()> {
    let sig = &item.sig;
    if !sig.inputs.is_empty() {
        return Err(syn::Error::new(
            sig.inputs.span(),
            "a task step takes no arguments: its state lives in a `wellfed_sdk::Slot` static",
        ));
    }
    if !sig.generics.params.is_empty() || sig.generics.where_clause.is_some() {
        return Err(syn::Error::new(
            sig.generics.span(),
            "a task entry is not generic: the stack audit finds it by name",
        ));
    }
    if let Some(token) = sig.asyncness.map(|t| t.span) {
        return Err(syn::Error::new(token, "a task step is not async"));
    }
    if let Some(token) = sig.unsafety.map(|t| t.span) {
        return Err(syn::Error::new(token, "a task step is safe to call"));
    }
    let returns_u32 = matches!(
        &sig.output,
        ReturnType::Type(_, ty) if matches!(&**ty, Type::Path(p) if p.qself.is_none() && p.path.is_ident("u32"))
    );
    if !returns_u32 {
        return Err(syn::Error::new(
            sig.output.span(),
            "a task step returns `u32`: the milliseconds to the next step, or zero to end",
        ));
    }
    Ok(())
}
