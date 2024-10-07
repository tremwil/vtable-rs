use proc_macro::TokenStream;
use proc_macro2::Span;
use quote::{quote, ToTokens};
use syn::{
    parse_quote, punctuated::Punctuated, Abi, BareFnArg, FnArg, Ident, ItemTrait, Lifetime, LitStr,
    Pat, Signature, Token, TraitItem, TraitItemFn, Type, TypeBareFn, TypeParamBound, TypeReference,
};

fn check_restrictions(trait_def: &ItemTrait) {
    // First, make sure we support the trait
    if trait_def.generics.lt_token.is_some() {
        panic!("vtable trait cannot be given a lifetime")
    }
    if !trait_def.generics.params.empty_or_trailing() {
        panic!("vtable traits do not support generic parameters yet")
    }
    if trait_def.auto_token.is_some() {
        panic!("vtable trait cannot be auto")
    }
    if trait_def.unsafety.is_some() {
        panic!("vtable trait cannot be unsafe")
    }
    if trait_def.supertraits.len() > 1 {
        panic!("vtable trait can only have a single supertrait")
    }
    if trait_def.items.is_empty() {
        panic!("vtable trait must contain at least one function")
    }
}

fn extract_base_trait(trait_def: &ItemTrait) -> Vec<proc_macro2::TokenStream> {
    match trait_def.supertraits.first() {
        None => None,
        Some(TypeParamBound::Trait(t)) => Some(t.to_token_stream()),
        Some(_) => panic!(
            "vtable trait's bounds must be a single trait representing the base class's vtable."
        ),
    }
    .into_iter()
    .collect()
}

fn set_method_abis(trait_def: &mut ItemTrait, abi: &str) {
    for item in trait_def.items.iter_mut() {
        if let TraitItem::Fn(fun) = item {
            // Add "extern C" ABI to the function if not present
            fun.sig.abi.get_or_insert(Abi {
                extern_token: Token![extern](Span::call_site()),
                name: Some(LitStr::new(abi, Span::call_site())),
            });
        }
        else {
            panic!("vtable trait can only contain functions")
        }
    }
}

fn trait_fn_to_bare_fn(fun: &TraitItemFn) -> TypeBareFn {
    let lifetimes = fun
        .sig
        .generics
        .lifetimes()
        .map(|lt| syn::GenericParam::Lifetime(lt.to_owned()));

    TypeBareFn {
        lifetimes: syn::parse2(quote! { for <#(#lifetimes),*> }).unwrap(),
        unsafety: fun.sig.unsafety,
        abi: fun.sig.abi.clone(),
        fn_token: Token![fn](Span::call_site()),
        paren_token: fun.sig.paren_token,
        inputs: {
            let mut inputs = Punctuated::new();
            let mut has_ref_receiver = false;
            for input in fun.sig.inputs.iter() {
                inputs.push(match input {
                    FnArg::Receiver(r) => {
                        has_ref_receiver = r.reference.is_some();
                        BareFnArg {
                            attrs: r.attrs.clone(),
                            name: Some((
                                Ident::new("this", Span::call_site()),
                                Token![:](Span::call_site()),
                            )),
                            ty: Type::Reference(TypeReference {
                                and_token: Token![&](Span::call_site()),
                                lifetime: r.lifetime().cloned(),
                                mutability: r.mutability,
                                elem: Box::new(parse_quote!(T)),
                            }),
                        }
                    }
                    FnArg::Typed(arg) => BareFnArg {
                        attrs: arg.attrs.clone(),
                        name: match arg.pat.as_ref() {
                            Pat::Ident(ident) => {
                                Some((ident.ident.clone(), Token![:](Span::call_site())))
                            }
                            _ => None,
                        },
                        ty: *arg.ty.to_owned(),
                    },
                });
            }
            if !has_ref_receiver {
                panic!(
                    "vtable trait method \"{0}\" must have &self or &mut self parameter",
                    fun.sig.ident.to_string()
                )
            }
            inputs
        },
        variadic: None,
        output: fun.sig.output.clone(),
    }
}

// TODO (WIP): Handle all lifetime edge cases before implementing
fn sig_to_vtable_thunk(sig: &Signature) -> proc_macro2::TokenStream {
    let (receiver_mut, receiver_lt) = match sig.inputs.first() {
        Some(FnArg::Receiver(r)) => (r.mutability.clone(), r.lifetime().cloned()),
        _ => unreachable!(),
    };

    let self_arg: FnArg = syn::parse2(quote! { &self }).unwrap();
    let t_arg: FnArg = syn::parse2(quote! { this: & #receiver_lt #receiver_mut T }).unwrap();

    let mut with_t = sig.clone();

    *with_t.inputs.first_mut().unwrap() = self_arg;
    with_t.inputs.insert(1, t_arg);
    with_t.abi = None; // No need for an ABI on the thunk method

    let ident = &sig.ident;
    let arg_idents = with_t.inputs.iter().skip(1).map(|arg| match arg {
        FnArg::Typed(pt) => match pt.pat.as_ref() {
            Pat::Ident(ident_pat) => ident_pat.ident.clone(),
            _ => unreachable!(),
        },
        _ => unreachable!(),
    });

    quote! {
        #[inline]
        pub #with_t {
            (self.#ident)(#(#arg_idents),*)
        }
    }
}

/// Attribute proc macro that can be used to turn a dyn-compatible trait definition
/// into a C++ compatible vtable definition.
///
/// For example, say we have a C++ abstract class of the form
/// ```cpp
/// struct Obj {
///     uint32_t field;
///
///     virtual ~Obj() = default;
///     virtual uint32_t method(uint32_t arg) const noexcept = 0;
/// };
/// ```
///
/// This macro then allows us to represent `Obj`'s virtual function table in Rust
/// and provide our own implementations:
///
/// ```rs
/// use vtable::{vtable, VPtr};
///
/// #[vtable]
/// pub trait ObjVmt {
///     fn destructor(&mut self) {
///         // We can provide a default implementation too!
///     }
///     fn method(&self, arg: u32) -> u32;
/// }
///
/// // VPtr implements Default for types that implement the trait, and provides
/// // a compile-time generated vtable!
/// #[derive(Default)]
/// #[repr(C)]
/// struct RustObj {
///     vftable: VPtr<dyn ObjVmt, Self>,
///     field: u32
/// }
///
/// impl ObjVmt for RustObj {
///     extern "C" fn method(&self, arg: u32) -> u32 {
///         self.field + arg
///     }
/// }
///
/// ```
///
/// `RustObj` could then be passed to a C++ function that takes in a pointer to `Obj`.
///
/// The macro supports single inhertiance through a single trait bound, e.g.
///
/// ```rs
/// #[vtable]
/// pub trait DerivedObjVmt: ObjVmt {
///     unsafe fn additional_method(&mut self, s: *const c_char);
/// }
/// ```
///
/// The vtable layout is fully typed and can be accessed as `<dyn TraitName as VmtLayout>::Layout<T>`.
/// A `VPtr` can be `Deref`'d into it to obtain the bare function pointers and thus call through
/// the vtable directly:
///
/// ```rs
/// let obj = RustObj::default();
/// let method_impl = obj.vftable.method; // extern "C" fn(&RustObj, u32) -> u32
/// let call_result = method_impl(obj, 42);
/// ```
#[proc_macro_attribute]
pub fn vtable(_attr: TokenStream, item: TokenStream) -> TokenStream {
    let mut trait_def: ItemTrait = syn::parse(item).unwrap();

    check_restrictions(&trait_def);

    let base_trait = extract_base_trait(&trait_def);

    // Add 'static lifetime bound to the trait
    trait_def.supertraits.push(TypeParamBound::Lifetime(Lifetime::new(
        "'static",
        Span::call_site(),
    )));

    // TODO: generate a #[cfg] to switch to fastcall for x86 windows support
    set_method_abis(&mut trait_def, "C");

    let layout_ident = Ident::new(&(trait_def.ident.to_string() + "Layout"), Span::call_site());
    let signatures: Vec<_> = trait_def
        .items
        .iter()
        .filter_map(|item| {
            if let TraitItem::Fn(fun) = item {
                Some(&fun.sig)
            }
            else {
                None
            }
        })
        .collect();

    let trait_ident = &trait_def.ident;
    let trait_vis = &trait_def.vis;
    let fn_idents: Vec<_> = signatures.iter().map(|sig| &sig.ident).collect();
    let bare_fns = trait_def.items.iter().filter_map(|item| match item {
        TraitItem::Fn(fun) => Some(trait_fn_to_bare_fn(fun)),
        _ => None,
    });

    // Create token stream with base layout declaration if a base trait is present
    let base_decl = if base_trait.is_empty() {
        proc_macro2::TokenStream::new()
    }
    else {
        quote! { _base: self._base, }
    };

    let base_deref_impl = match base_trait.first() {
        None => proc_macro2::TokenStream::new(),
        Some(base) => quote! {
            impl<T: 'static> ::core::ops::Deref for #layout_ident<T> {
                type Target = <dyn #base as ::vtable::VmtLayout>::Layout<T>;

                fn deref(&self) -> &Self::Target {
                    &self._base
                }
            }
            impl<T: 'static> ::core::ops::DerefMut for #layout_ident<T> {
                fn deref_mut(&mut self) -> &mut Self::Target {
                    &mut self._base
                }
            }
        },
    };

    // TODO: Figure out 100% reliable strategy to adjust lifetimes
    // so that lifetime inference works as expected in the trait definition
    //let thunk_impls = signatures.iter().map(|&s| sig_to_vtable_thunk(s));

    let mut tokens = trait_def.to_token_stream();
    tokens.extend(quote! {
        #[repr(C)]
        #trait_vis struct #layout_ident<T: 'static> {
            #(_base: <dyn #base_trait as ::vtable::VmtLayout>::Layout<T>,)*
            #(#fn_idents: #bare_fns,)*
        }

        // impl<T: 'static> #layout_ident<T> {
        //     #(#thunk_impls)*
        // }

        impl<T> ::core::clone::Clone for #layout_ident<T> {
            fn clone(&self) -> Self {
                Self {
                    #base_decl
                    #(#fn_idents: self.#fn_idents),*
                }
            }
        }
        impl<T> ::core::marker::Copy for #layout_ident<T> {}

        #base_deref_impl

        unsafe impl ::vtable::VmtLayout for dyn #trait_ident {
            type Layout<T: 'static> = #layout_ident<T>;
        }

        impl<T: #trait_ident> ::vtable::VmtInstance<T> for dyn #trait_ident {
            const VTABLE: &'static Self::Layout<T> = &#layout_ident {
                #(_base: *<dyn #base_trait as ::vtable::VmtInstance<T>>::VTABLE,)*
                #(#fn_idents: <T as #trait_ident>::#fn_idents),*
            };
        }
    });

    tokens.into()
}
