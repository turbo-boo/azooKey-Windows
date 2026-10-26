use proc_macro::TokenStream;
use quote::quote;
use syn::{
    parse_macro_input, parse_quote, Error, GenericArgument, ItemFn, PathArguments, ReturnType, Type,
};

fn extract_ok_type(return_type: &ReturnType) -> Result<&Type, TokenStream> {
    if let ReturnType::Type(_, ty) = return_type {
        if let Type::Path(type_path) = &**ty {
            if let Some(segment) = type_path.path.segments.last() {
                if segment.ident == "Result" {
                    if let PathArguments::AngleBracketed(args) = &segment.arguments {
                        if let Some(GenericArgument::Type(ok_type)) = args.args.first() {
                            return Ok(ok_type);
                        }
                    }
                }
            }
        }
        Err(
            Error::new_spanned(ty, "Expected a Result<T, anyhow::Error> return type")
                .to_compile_error()
                .into(),
        )
    } else {
        Err(
            Error::new_spanned(return_type, "Expected a Result return type")
                .to_compile_error()
                .into(),
        )
    }
}

#[proc_macro_attribute]
/// This macro wraps a function that returns a Result with an `anyhow::Result`.
///
/// If the function returns `anyhow::Result<OkType>`, it will be converted to `windows::core::Result<OkType>`.
/// The function's other attributes (e.g. `#[tracing::instrument]`) are kept.
/// An error that is, or was caused by, a `windows::core::Error` keeps its
/// HRESULT; any other error becomes E_FAIL.
///
///
/// ```rust
/// #[macros::anyhow]
/// fn some_func() -> anyhow::Result<Sometype> {
///     Ok(Sometype)
/// }
///
/// // will be converted to
///
/// fn some_func() -> windows::core::Result<Sometype> {
///   let result: anyhow::Result<Sometype> = (|| Ok(Sometype))();
///   match result {
///     Ok(v) => Ok(v),
///     Err(e) => {
///       tracing::error!("Error: {:?}", e);
///       Err(/* the windows::core::Error in `e`, or E_FAIL */)
///     }
///   }
/// }
/// ```
pub fn anyhow(_: TokenStream, input: TokenStream) -> TokenStream {
    // parse the input function
    let input_fn = parse_macro_input!(input as ItemFn);

    // get the attributes, visibility and body
    let attrs = &input_fn.attrs;
    let vis = &input_fn.vis;
    let fn_body = &input_fn.block;

    // check if the function has a return type
    let output = match &input_fn.sig.output {
        ReturnType::Type(_, _ty) => {
            let result = extract_ok_type(&input_fn.sig.output);

            match result {
                Ok(ok_type) => ok_type,
                Err(err) => return err,
            }
        }
        _ => {
            return Error::new_spanned(&input_fn.sig, "Expected a Result return type")
                .to_compile_error()
                .into();
        }
    };

    let result_type = match &input_fn.sig.output {
        ReturnType::Type(_, ty) => ty,
        ReturnType::Default => unreachable!("checked above"),
    };
    let mut sig = input_fn.sig.clone();
    sig.output = parse_quote!(-> ::windows::core::Result<#output>);

    // generate the new function
    let generated = quote! {
        #(#attrs)*
        #vis #sig {
            let result: #result_type = (|| #fn_body)();

            match result {
                Ok(v) => Ok(v),
                Err(e) => {
                    let e: ::anyhow::Error = e.into();
                    tracing::error!("Error: {:?}", e);
                    // A success code must not reach the caller as an error:
                    // it would read the out parameters as valid.
                    match e.downcast_ref::<::windows::core::Error>() {
                        Some(error) if error.code().is_err() => Err(error.clone()),
                        _ => Err(::windows::core::Error::from(
                            ::windows::Win32::Foundation::E_FAIL,
                        )),
                    }
                }
            }
        }
    };

    generated.into()
}
