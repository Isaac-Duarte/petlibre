//! `#[derive(Record)]`: a record's `PL…1` line body, `text key=value …`.
//!
//! The bench and QA tools parse these lines, so the derive writes exactly
//! what a hand-written formatter would: each variant is one `write!` whose
//! format string the derive builds from the attributes.
//!
//! - On the type: `#[record(tag = "PLBLK1")]` also implements the `LogRecord`
//!   trait, whose `log` writes the whole line. A struct takes its leading text
//!   here too.
//! - On a variant: `#[record("text")]` is the text before the fields, and
//!   `#[record(with = path)]` hands the variant to
//!   `fn(&Self, &mut Formatter) -> fmt::Result` for a line the attributes
//!   cannot express.
//! - On a field: `rename = "key"`, `with = path` (`path(value)` is
//!   displayed), `fmt = "02x"` (the format spec) and `skip`. A `bool` is
//!   written `0` or `1`; an unnamed field is written without a key.
//!
//! Fields are bound by value, so a record's fields are `Copy`, as every
//! record a task logs is.

use proc_macro2::{Span, TokenStream};
use quote::{format_ident, quote};
use syn::parse::ParseStream;
use syn::spanned::Spanned;
use syn::{Attribute, Data, DeriveInput, Fields, LitStr, Path, Type};

pub(crate) fn expand(input: &DeriveInput) -> syn::Result<TokenStream> {
    let container = Attrs::parse(&input.attrs)?;
    container.only(match input.data {
        Data::Struct(_) => &["tag", "text"],
        _ => &["tag"],
    })?;
    let name = &input.ident;
    let (impl_generics, ty_generics, where_clause) = input.generics.split_for_impl();

    let body = match &input.data {
        Data::Enum(data) => {
            let arms = data
                .variants
                .iter()
                .map(|variant| {
                    let attrs = Attrs::parse(&variant.attrs)?;
                    // A `with` variant writes its whole body.
                    attrs.only(if attrs.with.is_some() {
                        &["with"]
                    } else {
                        &["text"]
                    })?;
                    let ident = &variant.ident;
                    if let Some(with) = &attrs.with {
                        return Ok(quote! { Self::#ident { .. } => #with(self, f), });
                    }
                    let (pattern, write) = fields(&variant.fields, attrs.text.as_ref())?;
                    Ok(quote! { Self::#ident #pattern => #write, })
                })
                .collect::<syn::Result<Vec<_>>>()?;
            quote! { match *self { #(#arms)* } }
        }
        Data::Struct(data) => {
            let (pattern, write) = fields(&data.fields, container.text.as_ref())?;
            quote! {
                let Self #pattern = *self;
                #write
            }
        }
        Data::Union(_) => {
            return Err(syn::Error::new(
                name.span(),
                "a record is an enum or a struct",
            ));
        }
    };

    let display = quote! {
        impl #impl_generics ::core::fmt::Display for #name #ty_generics #where_clause {
            // Inline, so only an image that logs the record compiles it: the
            // linker keeps a string section whole, so an unused formatter's
            // strings would stay in every image.
            #[inline]
            fn fmt(&self, f: &mut ::core::fmt::Formatter<'_>) -> ::core::fmt::Result {
                #body
            }
        }
    };
    let Some(tag) = &container.tag else {
        return Ok(display);
    };

    check_tag(tag)?;
    let line = LitStr::new(&format!("{} {{}}\r\n", tag.value()), tag.span());
    Ok(quote! {
        #display

        impl #impl_generics ::wellfed_sdk::LogRecord for #name #ty_generics #where_clause {
            const TAG: &'static str = #tag;

            #[inline]
            fn log(&self) {
                ::wellfed_sdk::log(::core::format_args!(#line, self));
            }
        }
    })
}

/// A tag is `PL`, capitals or digits, then the format version `1`.
fn check_tag(tag: &LitStr) -> syn::Result<()> {
    let value = tag.value();
    let valid = value.len() > 3
        && value.starts_with("PL")
        && value.ends_with('1')
        && value
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit());
    if valid {
        Ok(())
    } else {
        Err(syn::Error::new(
            tag.span(),
            "a record tag looks like `PLBLK1`",
        ))
    }
}

/// The pattern binding `fields`, and the `write!` that formats them after
/// `text`.
fn fields(fields: &Fields, text: Option<&LitStr>) -> syn::Result<(TokenStream, TokenStream)> {
    let mut pieces: Vec<String> = text.map(|t| escape(&t.value())).into_iter().collect();
    let mut args = Vec::new();
    let mut bindings = Vec::new();

    for (index, field) in fields.iter().enumerate() {
        let attrs = Attrs::parse(&field.attrs)?;
        attrs.only(&["rename", "with", "fmt", "skip"])?;
        let binding = match &field.ident {
            Some(ident) => ident.clone(),
            None => format_ident!("field{index}"),
        };
        if attrs.skip {
            bindings.push(None);
            continue;
        }

        let spec = attrs
            .fmt
            .as_ref()
            .map_or(String::new(), |f| format!(":{}", f.value()));
        let key = match (&attrs.rename, &field.ident) {
            (Some(rename), _) => Some(rename.value()),
            (None, Some(ident)) => Some(ident.to_string().trim_start_matches("r#").to_owned()),
            (None, None) => None,
        };
        pieces.push(match key {
            Some(key) => format!("{}={{{spec}}}", escape(&key)),
            None => format!("{{{spec}}}"),
        });
        args.push(match &attrs.with {
            Some(with) => quote! { #with(#binding) },
            None if is_bool(&field.ty) => quote! { u8::from(#binding) },
            None => quote! { #binding },
        });
        bindings.push(Some(binding));
    }

    let pattern = match fields {
        Fields::Named(_) => {
            let named = bindings.iter().flatten();
            quote! { { #(#named,)* .. } }
        }
        Fields::Unnamed(_) => {
            let unnamed = bindings.iter().map(|b| match b {
                Some(binding) => quote! { #binding },
                None => quote! { _ },
            });
            quote! { ( #(#unnamed),* ) }
        }
        Fields::Unit => quote! {},
    };
    let format = pieces.join(" ");
    let write = if args.is_empty() {
        // `{{` in a literal with no arguments is written as is.
        let literal = format.replace("{{", "{").replace("}}", "}");
        quote! { f.write_str(#literal) }
    } else {
        quote! { ::core::write!(f, #format, #(#args),*) }
    };
    Ok((pattern, write))
}

fn escape(text: &str) -> String {
    text.replace('{', "{{").replace('}', "}}")
}

/// `bool`, however its path is spelled (`core::primitive::bool`).
fn is_bool(ty: &Type) -> bool {
    matches!(ty, Type::Path(p) if p.qself.is_none()
        && p.path.segments.last().is_some_and(|s| s.ident == "bool" && s.arguments.is_none()))
}

/// Every `#[record(…)]` on one item: an optional leading text, then
/// `key = value` and flags.
#[derive(Default)]
struct Attrs {
    /// The keys given, to refuse one where it means nothing.
    given: Vec<(String, Span)>,
    text: Option<LitStr>,
    tag: Option<LitStr>,
    with: Option<Path>,
    rename: Option<LitStr>,
    fmt: Option<LitStr>,
    skip: bool,
}

impl Attrs {
    fn parse(attrs: &[Attribute]) -> syn::Result<Self> {
        let mut out = Self::default();
        for attr in attrs.iter().filter(|a| a.path().is_ident("record")) {
            attr.parse_args_with(|input: ParseStream<'_>| out.parse_args(input))?;
        }
        Ok(out)
    }

    fn parse_args(&mut self, input: ParseStream<'_>) -> syn::Result<()> {
        if input.peek(LitStr) {
            let text: LitStr = input.parse()?;
            if self.text.is_some() {
                return Err(syn::Error::new(text.span(), "the text is given twice"));
            }
            self.given.push(("text".into(), text.span()));
            self.text = Some(text);
            if input.is_empty() {
                return Ok(());
            }
            input.parse::<syn::Token![,]>()?;
        }
        let parser = syn::meta::parser(|meta| {
            let key = meta
                .path
                .get_ident()
                .map(ToString::to_string)
                .ok_or_else(|| meta.error("expected a record attribute"))?;
            if self.given.iter().any(|(given, _)| *given == key) {
                return Err(meta.error(format!("`{key}` is given twice")));
            }
            self.given.push((key.clone(), meta.path.span()));
            match key.as_str() {
                "tag" => self.tag = Some(meta.value()?.parse()?),
                "with" => self.with = Some(meta.value()?.parse()?),
                "rename" => self.rename = Some(meta.value()?.parse()?),
                "fmt" => self.fmt = Some(meta.value()?.parse()?),
                "skip" => self.skip = true,
                _ => return Err(meta.error("unknown record attribute")),
            }
            Ok(())
        });
        let rest: TokenStream = input.parse()?;
        syn::parse::Parser::parse2(parser, rest)
    }

    /// Refuses any key given here that is not in `allowed`.
    fn only(&self, allowed: &[&str]) -> syn::Result<()> {
        match self
            .given
            .iter()
            .find(|(key, _)| !allowed.contains(&key.as_str()))
        {
            Some((key, span)) => Err(syn::Error::new(
                *span,
                format!("`{key}` does not apply here; expected one of {allowed:?}"),
            )),
            None => Ok(()),
        }
    }
}
