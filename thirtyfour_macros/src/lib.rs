extern crate proc_macro;
use itertools::izip;
use proc_macro::TokenStream;
use proc_macro2::Literal;
use proc_macro_error::abort;
use quote::{format_ident, quote};
use std::collections::HashSet;
use syn::{
    Attribute, Data, DeriveInput, Fields, GenericArgument, Lit, Meta, MetaNameValue, NestedMeta,
    Path, PathArguments, PathSegment, Type,
};

#[proc_macro_derive(Component, attributes(base, by))]
#[proc_macro_error::proc_macro_error]
pub fn derive_component_fn(input: TokenStream) -> TokenStream {
    let ast: DeriveInput = syn::parse(input).unwrap();
    let ident = ast.ident;

    let (base, prefields, fields) = match ast.data {
        Data::Struct(s) => {
            // TODO:
            match s.fields {
                Fields::Named(nf) => {
                    // TODO:
                    let field_names =
                        nf.named.iter().map(|x| x.ident.as_ref().expect("unknown field name"));
                    let field_types = nf.named.iter().map(|x| &x.ty);
                    let field_attrs = nf.named.iter().map(|x| &x.attrs);
                    let mut fields = Vec::new();
                    let mut prefields = Vec::new();
                    let mut base_field = None;
                    for (field_name, field_type, attrs) in
                        izip!(field_names, field_types, field_attrs)
                    {
                        // Find base element.
                        let is_base = attrs.iter().any(|x| x.path.is_ident("base"));
                        if (base_field.is_none() && field_name == "base") || is_base {
                            match field_type {
                                Type::Path(p) => {
                                    if !p.path.is_ident("WebElement") {
                                        abort! { p, "base field must be a WebElement" }
                                    }
                                }
                                t => abort! { t, "base field must be a WebElement" },
                            }
                            base_field = Some(field_name.clone());
                            continue;
                        }

                        // Get attributes
                        let mut by_ident = None;
                        for attr in attrs {
                            if attr.path.is_ident("by") {
                                if let Ok(x) = ByTokens::try_from(attr) {
                                    by_ident = Some(x);
                                }
                            }
                        }

                        // Initializer
                        let (predef, def) = match field_type {
                            Type::Path(p) => {
                                match by_ident {
                                    Some(by) => {
                                        // Has a #[by()] attribute.
                                        if by.is_multi() || is_multi_resolver(&p.path) {
                                            let multi_args: MultiResolverArgs = by.into();
                                            let multi_constructor: proc_macro2::TokenStream =
                                                multi_args.into();

                                            let ty = fix_type(p.path.clone());

                                            let predef = quote! {
                                                let #field_name = #ty::#multi_constructor
                                            };
                                            let def = quote! {
                                                #field_name
                                            };
                                            (Some(predef), def)
                                        } else {
                                            let single_args: SingleResolverArgs = by.into();
                                            let single_constructor: proc_macro2::TokenStream =
                                                single_args.into();

                                            let ty = fix_type(p.path.clone());

                                            let predef = quote! {
                                                let #field_name = #ty::#single_constructor
                                            };
                                            let def = quote! {
                                                #field_name
                                            };
                                            (Some(predef), def)
                                        }
                                    }
                                    _ => {
                                        // No #[by()] attribute.
                                        let def = quote! {
                                            # field_name: Default::default()
                                        };
                                        (None, def)
                                    }
                                }
                            }
                            _ => {
                                let def = quote! {
                                    #field_name: Default::default()
                                };
                                (None, def)
                            }
                        };

                        if let Some(pre) = predef {
                            prefields.push(pre);
                        }

                        fields.push(def);
                    }
                    (base_field, prefields, fields)
                }
                _ => panic!("Tuple or unit structs not supported"),
            }
        }
        Data::Enum(_) | Data::Union(_) => {
            panic!("Component attribute not supported for enums or unions")
        }
    };
    let base = base.unwrap_or_else(|| {
        abort!(
            ident,
            "base field not found. Add the #[base] attribute for the base WebElement field"
        )
    });

    let gen = quote! {
        impl #ident {
            pub fn new(base: thirtyfour::WebElement) -> Self {
                #(#prefields)*
                Self {
                    #base: base,
                    #(#fields,)*
                }
            }
        }

        #[automatically_derived]
        impl From<thirtyfour::WebElement> for #ident {
            fn from(elem: thirtyfour::WebElement) -> Self {
                Self::new(elem)
            }
        }

        #[automatically_derived]
        impl Component for #ident {
            fn base_element(&self) -> thirtyfour::WebElement {
                self.#base.clone()
            }
        }
    };
    gen.into()
}

#[derive(Debug, Clone)]
struct WaitOptions {
    timeout_ms: u32,
    interval_ms: u32,
}

#[derive(Debug)]
enum ByToken {
    Id(Literal),
    Tag(Literal),
    LinkText(Literal),
    Css(Literal),
    XPath(Literal),
    Name(Literal),
    Multi,
    AllowEmpty,
    First,
    IgnoreErrors,
    Description(String),
    Wait(WaitOptions),
    CustomFn(String),
}

impl ByToken {
    /// Helper for making sure the right things are mutually exclusive.
    fn get_unique_type(&self) -> &str {
        match &self {
            ByToken::Id(_)
            | ByToken::Tag(_)
            | ByToken::LinkText(_)
            | ByToken::Css(_)
            | ByToken::XPath(_)
            | ByToken::Name(_) => "selector",
            ByToken::Multi => "multi",
            ByToken::AllowEmpty => "allow_empty",
            ByToken::First => "first",
            ByToken::IgnoreErrors => "ignore_errors",
            ByToken::Description(_) => "description",
            ByToken::Wait(_) => "wait",
            ByToken::CustomFn(_) => "custom",
        }
    }

    fn get_disallowed_types(&self) -> Vec<&str> {
        match &self {
            ByToken::AllowEmpty => vec!["custom"],
            ByToken::First => vec!["multi", "custom"],
            ByToken::IgnoreErrors => vec!["custom"],
            ByToken::Description(_) => vec!["custom"],
            ByToken::Wait(_) => vec!["custom"],
            ByToken::CustomFn(_) => {
                vec!["multi", "first", "ignore_errors", "description", "wait", "allow_empty"]
            }
            _ => vec![],
        }
    }
}

impl TryFrom<Meta> for ByToken {
    type Error = TokenStream;

    fn try_from(value: Meta) -> Result<Self, Self::Error> {
        match value {
            Meta::Path(p) => match p {
                k if k.is_ident("multi") => Ok(ByToken::Multi),
                k if k.is_ident("allow_empty") => Ok(ByToken::AllowEmpty),
                k if k.is_ident("first") => Ok(ByToken::First),
                k if k.is_ident("ignore_errors") => Ok(ByToken::IgnoreErrors),
                e => abort! { e, format!("unknown attribute {e:?}") },
            },
            Meta::List(l) => match l.path {
                // wait(timeout_ms = u32, interval_ms = u32)
                p if p.is_ident("wait") => {
                    let mut timeout: Option<u32> = None;
                    let mut interval: Option<u32> = None;
                    for n in l.nested.into_iter() {
                        match n {
                            NestedMeta::Meta(Meta::NameValue(MetaNameValue {
                                path,
                                lit,
                                ..
                            })) => match (path, lit) {
                                (k, Lit::Int(v)) if k.is_ident("timeout_ms") => {
                                    assert!(timeout.is_none(), "cannot specify timeout twice");
                                    timeout = Some(
                                        v.base10_parse()
                                            .expect("invalid timeout_ms value (must be u32)"),
                                    );
                                }
                                (k, Lit::Int(v)) if k.is_ident("interval_ms") => {
                                    assert!(interval.is_none(), "cannot specify interval twice");
                                    interval = Some(
                                        v.base10_parse()
                                            .expect("invalid interval_ms value (must be u32)"),
                                    );
                                }
                                e => {
                                    abort! { p , format!("unknown attribute {e:?} (must be timeout_ms or interval_ms)") }
                                }
                            },
                            e => {
                                abort! { p, format!("unknown attribute {e:?} (format should be `wait(timeout_ms=30000, interval_ms=500)`)") }
                            }
                        }
                    }

                    match (timeout, interval) {
                        (Some(t), Some(i)) => Ok(ByToken::Wait(WaitOptions {
                            timeout_ms: t,
                            interval_ms: i,
                        })),
                        _ => {
                            abort! { p, "wait attribute requires the following args: timeout_ms, interval_ms" }
                        }
                    }
                }
                e => abort! { e, format!("unknown attribute: {e:?}") },
            },
            Meta::NameValue(MetaNameValue {
                path,
                lit,
                ..
            }) => match (path, lit) {
                (k, Lit::Str(v)) if k.is_ident("id") => Ok(ByToken::Id(v.token())),
                (k, Lit::Str(v)) if k.is_ident("tag") => Ok(ByToken::Tag(v.token())),
                (k, Lit::Str(v)) if k.is_ident("link") => Ok(ByToken::LinkText(v.token())),
                (k, Lit::Str(v)) if k.is_ident("css") => Ok(ByToken::Css(v.token())),
                (k, Lit::Str(v)) if k.is_ident("xpath") => Ok(ByToken::XPath(v.token())),
                (k, Lit::Str(v)) if k.is_ident("name") => Ok(ByToken::Name(v.token())),
                (k, Lit::Str(v)) if k.is_ident("description") => {
                    Ok(ByToken::Description(v.value()))
                }
                (k, Lit::Str(v)) if k.is_ident("custom") => Ok(ByToken::CustomFn(v.value())),
                (k, ..) => abort! { k, format!("unknown attribute: {k:?}") },
            },
        }
    }
}

struct ByTokens {
    tokens: Vec<ByToken>,
}

impl ByTokens {
    pub fn validate(&self) -> Result<(), String> {
        let mut unique_tokens = HashSet::new();
        for token in self.tokens.iter() {
            let t = token.get_unique_type();
            if unique_tokens.contains(t) {
                return Err(format!("duplicate token '{t}' (cannot specify multiple)"));
            }
            unique_tokens.insert(t);
        }
        for token in self.tokens.iter() {
            let disallowed = token.get_disallowed_types();
            for t in disallowed {
                if unique_tokens.contains(t) {
                    let unique = token.get_unique_type();
                    return Err(format!("cannot specify '{unique}' with '{t}'"));
                }
            }
        }

        Ok(())
    }

    /// Extract just the "By"-specific part of the tokens.
    ///
    /// For example, `name = "element-name"`.
    ///
    /// This removes the token from the vec.
    ///
    /// This will also panic if more than one such token exists.
    pub fn take_quote(&mut self) -> proc_macro2::TokenStream {
        let mut ret = Vec::new();
        let tokens_in = std::mem::take(&mut self.tokens);
        for token in tokens_in.into_iter() {
            match token {
                ByToken::Id(id) => ret.push(quote! { By::Id(#id) }),
                ByToken::Tag(tag) => ret.push(quote! { By::Tag(#tag) }),
                ByToken::LinkText(text) => ret.push(quote! { By::LinkText(#text) }),
                ByToken::Css(css) => ret.push(quote! { By::Css(#css) }),
                ByToken::XPath(xpath) => ret.push(quote! { By::XPath(#xpath) }),
                ByToken::Name(name) => ret.push(quote! { By::Name(#name) }),
                t => self.tokens.push(t),
            }
        }

        match ret.len() {
            0 => panic!("no selector found"),
            1 => ret.into_iter().next().unwrap(),
            _ => panic!("multiple selectors are not supported"),
        }
    }

    pub fn is_multi(&self) -> bool {
        self.tokens.iter().any(|x| matches!(&x, ByToken::Multi))
    }

    pub fn take_one<F, T>(&mut self, f: F) -> Option<T>
    where
        F: Fn(&ByToken) -> Option<T>,
    {
        let mut pos = None;
        let mut value = None;
        for (i, t) in self.tokens.iter().enumerate() {
            if let Some(v) = f(t) {
                pos = Some(i);
                value = Some(v);
                break;
            }
        }

        match (pos, value) {
            (Some(p), Some(v)) => {
                self.tokens.remove(p);
                Some(v)
            }
            _ => None,
        }
    }

    pub fn take_multi(&mut self) -> Option<bool> {
        self.take_one(|x| match x {
            ByToken::Multi => Some(true),
            _ => None,
        })
    }

    pub fn take_first(&mut self) -> Option<bool> {
        self.take_one(|x| match x {
            ByToken::First => Some(true),
            _ => None,
        })
    }

    pub fn take_allow_empty(&mut self) -> Option<bool> {
        self.take_one(|x| match x {
            ByToken::AllowEmpty => Some(true),
            _ => None,
        })
    }

    pub fn take_ignore_errors(&mut self) -> Option<bool> {
        self.take_one(|x| match x {
            ByToken::IgnoreErrors => Some(true),
            _ => None,
        })
    }

    pub fn take_description(&mut self) -> Option<String> {
        self.take_one(|x| match x {
            ByToken::Description(d) => Some(d.clone()),
            _ => None,
        })
    }

    pub fn take_wait_options(&mut self) -> Option<WaitOptions> {
        self.take_one(|x| match x {
            ByToken::Wait(w) => Some(w.clone()),
            _ => None,
        })
    }

    pub fn take_custom(&mut self) -> Option<String> {
        self.take_one(|x| match x {
            ByToken::CustomFn(f) => Some(f.clone()),
            _ => None,
        })
    }
}

/// Parse an attribute into tokens.
impl TryFrom<&Attribute> for ByTokens {
    type Error = TokenStream;

    fn try_from(attr: &Attribute) -> Result<Self, Self::Error> {
        let meta = attr.parse_meta().expect("invalid arg format");
        let mut by_tokens = ByTokens {
            tokens: Vec::new(),
        };
        match meta {
            Meta::List(l) => {
                if !l.path.is_ident("by") {
                    abort!(l, "only 'by' attributes are supported here");
                }
                let args: Vec<NestedMeta> = l.nested.into_iter().collect();
                for arg in &args {
                    let token = match arg {
                        NestedMeta::Meta(meta) => ByToken::try_from(meta.clone())?,
                        t => {
                            abort! { t, format!("unrecognised token: {t:?}") }
                        }
                    };
                    by_tokens.tokens.push(token);
                    by_tokens.validate().unwrap_or_else(|e| {
                        abort! { arg , format!("{e}")}
                    });
                }
            }
            _ => panic!("unrecognised by argument format"),
        }

        Ok(by_tokens)
    }
}

/// Return true if this path should be treated as a multi element resolver.
fn is_multi_resolver(path: &Path) -> bool {
    // First check for the type alias.
    if path.is_ident("ElementResolverMulti") {
        true
    } else {
        if let Some(x) = path.segments.last() {
            if x.ident == "ElementResolver" {
                // If we have `ElementResolver<Vec<T>>` then use multi.
                if let PathArguments::AngleBracketed(x) = &x.arguments {
                    for arg in &x.args {
                        if let GenericArgument::Type(Type::Path(t)) = arg {
                            let idents_of_path =
                                t.path.segments.iter().fold(String::new(), |mut acc, v| {
                                    acc.push_str(&v.ident.to_string());
                                    acc.push(':');
                                    acc
                                });

                            return ["Vec:", "vec:Vec:", "std:vec:Vec:", "alloc:vec:Vec:"]
                                .into_iter()
                                .any(|x| idents_of_path == x);
                        }
                    }
                }
            }
        }

        false
    }
}

enum SingleResolverArgs {
    CustomFn(String),
    Opts {
        by: proc_macro2::TokenStream,
        first: Option<bool>,
        ignore_errors: Option<bool>,
        description: Option<String>,
        wait: Option<WaitOptions>,
    },
}

impl From<ByTokens> for SingleResolverArgs {
    fn from(mut t: ByTokens) -> Self {
        let s = match t.take_custom() {
            Some(f) => Self::CustomFn(f),
            None => Self::Opts {
                by: t.take_quote(),
                first: t.take_first(),
                ignore_errors: t.take_ignore_errors(),
                description: t.take_description(),
                wait: t.take_wait_options(),
            },
        };

        assert!(t.tokens.is_empty(), "unrecognised args: {:?}", t.tokens);
        s
    }
}

impl Into<proc_macro2::TokenStream> for SingleResolverArgs {
    fn into(self) -> proc_macro2::TokenStream {
        match self {
            SingleResolverArgs::CustomFn(f) => {
                let f_ident = format_ident!("{f}");
                quote! {
                    new_custom(base.clone(), #f_ident);
                }
            }
            SingleResolverArgs::Opts {
                by,
                first,
                ignore_errors,
                description,
                wait,
            } => {
                let ignore_errors_ident = match ignore_errors {
                    Some(true) => {
                        format_ident!("Some(true)")
                    }
                    _ => format_ident!("None"),
                };
                let description_ident = match description {
                    Some(desc) => format_ident!("Some({desc})"),
                    None => format_ident!("None"),
                };
                let wait_ident = match wait {
                    Some(WaitOptions {
                        timeout_ms,
                        interval_ms,
                    }) => {
                        let timeout_ident = format_ident!("{timeout_ms}");
                        let interval_ident = format_ident!("{interval_ms}");
                        quote! {
                            thirtyfour::extensions::query::ElementQueryWaitOptions::Wait {
                                timeout: #timeout_ident,
                                interval: #interval_ident
                            }
                        }
                    }
                    None => quote! { None },
                };
                let opts_ident = quote! {
                    thirtyfour::extensions::query::ElementQueryOptions::default()
                        .set_ignore_errors(#ignore_errors_ident)
                        .set_description(#description_ident)
                        .set_wait(#wait_ident)
                };

                match first {
                    Some(true) => {
                        quote! {

                            new_first_opts(base.clone(), #by, #opts_ident);
                        }
                    }
                    _ => {
                        quote! {
                            new_single_opts(base.clone(), #by, #opts_ident);
                        }
                    }
                }
            }
        }
    }
}

enum MultiResolverArgs {
    CustomFn(String),
    Opts {
        by: proc_macro2::TokenStream,
        allow_empty: Option<bool>,
        ignore_errors: Option<bool>,
        description: Option<String>,
        wait: Option<WaitOptions>,
    },
}

impl From<ByTokens> for MultiResolverArgs {
    fn from(mut t: ByTokens) -> Self {
        t.take_multi(); // Not used here.
        let s = match t.take_custom() {
            Some(f) => Self::CustomFn(f),
            None => Self::Opts {
                by: t.take_quote(),
                allow_empty: t.take_allow_empty(),
                ignore_errors: t.take_ignore_errors(),
                description: t.take_description(),
                wait: t.take_wait_options(),
            },
        };

        assert!(t.tokens.is_empty(), "unrecognised args: {:?}", t.tokens);
        s
    }
}

impl Into<proc_macro2::TokenStream> for MultiResolverArgs {
    fn into(self) -> proc_macro2::TokenStream {
        match self {
            MultiResolverArgs::CustomFn(f) => {
                let f_ident = format_ident!("{f}");
                quote! {
                    new_custom(base.clone(), #f_ident);
                }
            }
            MultiResolverArgs::Opts {
                by,
                allow_empty,
                ignore_errors,
                description,
                wait,
            } => {
                let ignore_errors_ident = match ignore_errors {
                    Some(true) => {
                        format_ident!("Some(true)")
                    }
                    _ => format_ident!("None"),
                };
                let description_ident = match description {
                    Some(desc) => format_ident!("Some({desc})"),
                    None => format_ident!("None"),
                };
                let wait_ident = match wait {
                    Some(WaitOptions {
                        timeout_ms,
                        interval_ms,
                    }) => {
                        let timeout_ident = format_ident!("{timeout_ms}");
                        let interval_ident = format_ident!("{interval_ms}");
                        quote! {
                            thirtyfour::extensions::query::ElementQueryWaitOptions::Wait {
                                timeout: #timeout_ident,
                                interval: #interval_ident
                            }
                        }
                    }
                    None => quote! { None },
                };
                let opts_ident = quote! {
                    thirtyfour::extensions::query::ElementQueryOptions::default()
                        .set_ignore_errors(#ignore_errors_ident)
                        .set_description(#description_ident)
                        .set_wait(#wait_ident)
                };

                match allow_empty {
                    Some(true) => {
                        quote! {
                            new_allow_empty_opts(base.clone(), #by, #opts_ident);
                        }
                    }
                    _ => {
                        quote! {
                            new_not_empty_opts(base.clone(), #by, #opts_ident);
                        }
                    }
                }
            }
        }
    }
}

/// Converts GenericType<Args> to GenericType::<Args> in order to call ::new_*() on it.
///
/// Non-generic types will be returned as is.
fn fix_type(mut ty: Path) -> proc_macro2::TokenStream {
    let last = ty.segments.pop();
    match last {
        Some(pair) => {
            let (p, _) = pair.into_tuple();
            let ident = p.ident;
            let args = p.arguments;
            if args.is_empty() {
                ty.segments.push(PathSegment::from(ident));
                quote! { #ty }
            } else if ty.segments.is_empty() {
                quote! { #ident::# args }
            } else {
                quote! { #ty::#ident::#args }
            }
        }
        None => {
            quote! {}
        }
    }
}
