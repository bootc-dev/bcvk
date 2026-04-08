//! Proc-macro companion for the `itest` integration test framework.
//!
//! Provides the `#[itest::test]` attribute macro for registering
//! integration tests with less boilerplate than the declarative macros.

use proc_macro::TokenStream;
use quote::quote;
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::{Expr, Ident, ItemFn, Token};

/// Attribute arguments for `#[itest::test(...)]`.
struct TestArgs {
    privileged: bool,
    booted: bool,
    binary: Option<String>,
    itype: Option<String>,
    timeout: Option<String>,
    tags: Vec<String>,
    summary: Option<String>,
    needs_root: bool,
    needs_internet: bool,
    flaky: bool,
}

impl Default for TestArgs {
    fn default() -> Self {
        Self {
            privileged: false,
            booted: false,
            binary: None,
            itype: None,
            timeout: None,
            tags: Vec::new(),
            summary: None,
            needs_root: false,
            needs_internet: false,
            flaky: false,
        }
    }
}

/// A single key or key=value argument.
enum Arg {
    Flag(Ident),
    KeyValue(Ident, Expr),
}

impl Parse for Arg {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        let key: Ident = input.parse()?;
        if input.peek(Token![=]) {
            let _: Token![=] = input.parse()?;
            let value: Expr = input.parse()?;
            Ok(Arg::KeyValue(key, value))
        } else {
            Ok(Arg::Flag(key))
        }
    }
}

struct TestAttrArgs {
    args: Punctuated<Arg, Token![,]>,
}

impl Parse for TestAttrArgs {
    fn parse(input: ParseStream) -> syn::Result<Self> {
        Ok(TestAttrArgs {
            args: Punctuated::parse_terminated(input)?,
        })
    }
}

fn parse_args(attr: TokenStream) -> syn::Result<TestArgs> {
    if attr.is_empty() {
        return Ok(TestArgs::default());
    }

    let parsed: TestAttrArgs = syn::parse(attr)?;
    let mut result = TestArgs::default();

    for arg in parsed.args {
        match arg {
            Arg::Flag(ident) => {
                let name = ident.to_string();
                match name.as_str() {
                    "privileged" => result.privileged = true,
                    "booted" => result.booted = true,
                    "needs_root" => result.needs_root = true,
                    "needs_internet" => result.needs_internet = true,
                    "flaky" => result.flaky = true,
                    _ => {
                        return Err(syn::Error::new_spanned(
                            ident,
                            format!("unknown flag: {name}"),
                        ))
                    }
                }
            }
            Arg::KeyValue(ident, value) => {
                let name = ident.to_string();
                match name.as_str() {
                    "binary" => {
                        result.binary = Some(expr_to_string(&value)?);
                    }
                    "itype" => {
                        result.itype = Some(expr_to_string(&value)?);
                    }
                    "timeout" => {
                        result.timeout = Some(expr_to_string(&value)?);
                    }
                    "summary" => {
                        result.summary = Some(expr_to_string(&value)?);
                    }
                    "tags" => {
                        result.tags = expr_to_string_list(&value)?;
                    }
                    _ => {
                        return Err(syn::Error::new_spanned(
                            ident,
                            format!("unknown attribute: {name}"),
                        ))
                    }
                }
            }
        }
    }

    // Validate: privileged/booted require binary
    if (result.privileged || result.booted) && result.binary.is_none() {
        return Err(syn::Error::new(
            proc_macro2::Span::call_site(),
            "privileged/booted tests require `binary = \"...\"`",
        ));
    }

    Ok(result)
}

/// Extract a string literal from an expression.
fn expr_to_string(expr: &Expr) -> syn::Result<String> {
    match expr {
        Expr::Lit(lit) => {
            if let syn::Lit::Str(s) = &lit.lit {
                Ok(s.value())
            } else {
                Err(syn::Error::new_spanned(expr, "expected string literal"))
            }
        }
        _ => Err(syn::Error::new_spanned(expr, "expected string literal")),
    }
}

/// Extract a list of string literals from `["a", "b"]`.
fn expr_to_string_list(expr: &Expr) -> syn::Result<Vec<String>> {
    match expr {
        Expr::Array(arr) => arr.elems.iter().map(|e| expr_to_string(e)).collect(),
        _ => Err(syn::Error::new_spanned(
            expr,
            "expected array of string literals",
        )),
    }
}

/// Register a function as an itest integration test.
///
/// # Plain test
///
/// ```ignore
/// /// My test does something.
/// #[itest::test]
/// fn test_something() -> anyhow::Result<()> {
///     Ok(())
/// }
/// ```
///
/// # Privileged test (auto-dispatches to VM when not root)
///
/// ```ignore
/// #[itest::test(privileged, binary = "my-binary")]
/// fn privileged_check_root() -> anyhow::Result<()> {
///     Ok(())
/// }
/// ```
///
/// # Booted test (full bootc install-to-disk)
///
/// ```ignore
/// #[itest::test(booted, binary = "my-binary", itype = "u1.large")]
/// fn test_ostree() -> anyhow::Result<()> {
///     Ok(())
/// }
/// ```
///
/// # Metadata
///
/// ```ignore
/// #[itest::test(
///     timeout = "1h",
///     tags = ["slow", "network"],
///     needs_internet,
///     flaky,
///     summary = "A slow network test",
/// )]
/// fn slow_test() -> anyhow::Result<()> {
///     Ok(())
/// }
/// ```
#[proc_macro_attribute]
pub fn integration_test(attr: TokenStream, item: TokenStream) -> TokenStream {
    match test_impl(attr, item) {
        Ok(ts) => ts,
        Err(e) => e.to_compile_error().into(),
    }
}

fn test_impl(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args = parse_args(attr)?;
    let func: ItemFn = syn::parse(item)?;

    let is_async = func.sig.asyncness.is_some();

    let fn_name = &func.sig.ident;
    let fn_name_str = fn_name.to_string();

    // Generate the wrapper function name
    let wrapper_name = syn::Ident::new(&format!("__itest_wrap_{fn_name_str}"), fn_name.span());
    let slice_name = syn::Ident::new(
        &format!("__ITEST_{}", fn_name_str.to_uppercase()),
        fn_name.span(),
    );

    // Build TestMeta
    let timeout_expr = match &args.timeout {
        Some(t) => quote! { Some(#t) },
        None => quote! { None },
    };
    let summary_expr = match &args.summary {
        Some(s) => quote! { Some(#s) },
        None => quote! { None },
    };
    let tags_expr = if args.tags.is_empty() {
        quote! { &[] }
    } else {
        let tags = &args.tags;
        quote! { &[#(#tags),*] }
    };
    let needs_root = args.needs_root || args.privileged || args.booted;
    let needs_internet = args.needs_internet;
    let flaky = args.flaky;
    let isolation_expr = if args.booted {
        quote! { ::itest::Isolation::Machine }
    } else {
        quote! { ::itest::Isolation::None }
    };

    // How to call the test function — async fns need a tokio runtime.
    let call_expr = if is_async {
        quote! {
            ::itest::tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
                .expect("failed to build tokio runtime")
                .block_on(#fn_name())
        }
    } else {
        quote! { #fn_name() }
    };

    // Build the wrapper function body for privileged/booted tests
    let body = if args.privileged || args.booted {
        let binary = args.binary.as_ref().unwrap(); // validated above
        let dispatch_mode = if args.booted {
            quote! { ::itest::DispatchMode::Booted }
        } else {
            quote! { ::itest::DispatchMode::Privileged }
        };
        let itype_expr = match &args.itype {
            Some(t) => quote! { Some(#t) },
            None => quote! { None },
        };

        quote! {
            fn #wrapper_name() -> ::itest::TestResult {
                let vm_opts = ::itest::VmOptions { itype: #itype_expr, ..Default::default() };
                if ::itest::require_root(
                    #fn_name_str,
                    #binary,
                    #dispatch_mode,
                    &vm_opts,
                )?
                .is_some()
                {
                    return Ok(());
                }
                #call_expr.map_err(::std::convert::Into::into)
            }
        }
    } else {
        quote! {
            fn #wrapper_name() -> ::itest::TestResult {
                #call_expr.map_err(::std::convert::Into::into)
            }
        }
    };

    let output = quote! {
        #func

        #body

        #[::itest::linkme::distributed_slice(::itest::INTEGRATION_TESTS)]
        static #slice_name: ::itest::IntegrationTest = ::itest::IntegrationTest::with_meta(
            #fn_name_str,
            #wrapper_name,
            ::itest::TestMeta {
                timeout: #timeout_expr,
                needs_root: #needs_root,
                isolation: #isolation_expr,
                tags: #tags_expr,
                summary: #summary_expr,
                needs_internet: #needs_internet,
                flaky: #flaky,
            },
        );
    };

    Ok(output.into())
}
