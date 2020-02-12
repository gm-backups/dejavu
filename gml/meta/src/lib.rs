extern crate proc_macro;

use std::iter;
use std::collections::HashMap;
use proc_macro::TokenStream;
use proc_macro2;
use syn::{
    self, parse_quote, parenthesized, punctuated,
    ItemImpl, ImplItemMethod, Attribute, Signature, FnArg, PatType, ReturnType, Type, Ident
};
use syn::parse::{Parse, ParseStream, Result, Error};
use syn::visit_mut::VisitMut;
use syn::spanned::Spanned;
use quote::quote;

struct ItemBindings {
    functions: Vec<Function>,
    members: HashMap<Ident, Member>,
}

struct Function {
    name: Ident,
    receivers: Vec<Receiver>,
    parameters: Vec<Parameter>,
    rest: Option<()>,
    output: Return,
}

#[derive(Default)]
struct Member {
    getter: Option<Property>,
    setter: Option<Property>,
}

struct Property {
    name: Ident,
    receivers: Vec<Receiver>,
    entity: Option<()>,
    index: Option<()>,
    value: Option<Parameter>,
}

#[derive(Copy, Clone)]
enum Receiver {
    Self_,
    World,
}

#[derive(Copy, Clone)]
enum Parameter {
    Direct,
    Convert,
}

#[derive(Copy, Clone)]
enum Return {
    Value,
    Result,
}

impl ItemBindings {
    fn parse(item: &mut ItemImpl) -> std::result::Result<Self, Vec<Error>> {
        let mut bindings = ItemBindings {
            functions: Vec::new(),
            members: HashMap::new(),
        };
        let mut errors = Vec::new();

        {
            let mut visit = VisitBindings {
                bindings: &mut bindings,
                errors: &mut errors,
            };
            syn::visit_mut::visit_item_impl_mut(&mut visit, item);
        }

        if !errors.is_empty() {
            return Err(errors);
        }

        Ok(bindings)
    }
}

struct VisitBindings<'a> {
    bindings: &'a mut ItemBindings,
    errors: &'a mut Vec<Error>,
}

impl VisitMut for VisitBindings<'_> {
    fn visit_impl_item_method_mut(&mut self, item: &mut ImplItemMethod) {
        let sig = &item.sig;
        item.attrs.retain(|attr| !self.process_attribute(attr, &sig));
    }
}

impl VisitBindings<'_> {
    fn process_attribute(&mut self, attr: &Attribute, sig: &Signature) -> bool {
        if attr.path == parse_quote!(gml::function) {
            let function = match Function::parse(sig) {
                Ok(function) => function,
                Err(err) => {
                    self.errors.push(err);
                    return true;
                }
            };
            self.bindings.functions.push(function);
            true
        } else if attr.path == parse_quote!(gml::get) {
            let meta: PropertyMeta = match syn::parse2(attr.tokens.clone()) {
                Ok(meta) => meta,
                Err(err) => {
                    self.errors.push(err);
                    return true;
                }
            };
            let property = match Property::parse(sig) {
                Ok(property) => property,
                Err(err) => {
                    self.errors.push(err);
                    return true;
                }
            };
            let member = self.bindings.members.entry(meta.name.clone()).or_default();
            if member.getter.is_some() {
                self.errors.push(Error::new(meta.name.span(), "getter is defined multiple times"));
                return true;
            }
            member.getter = Some(property);
            true
        } else if attr.path == parse_quote!(gml::set) {
            let meta: PropertyMeta = match syn::parse2(attr.tokens.clone()) {
                Ok(meta) => meta,
                Err(err) => {
                    self.errors.push(err);
                    return true;
                }
            };
            let property = match Property::parse(sig) {
                Ok(property) => property,
                Err(err) => {
                    self.errors.push(err);
                    return true;
                }
            };
            let member = self.bindings.members.entry(meta.name.clone()).or_default();
            if member.setter.is_some() {
                self.errors.push(Error::new(meta.name.span(), "setter is defined multiple times"));
                return true;
            }
            member.setter = Some(property);
            true
        } else {
            false
        }
    }
}

struct PropertyMeta {
    name: Ident,
}

impl Parse for PropertyMeta {
    fn parse(input: ParseStream<'_>) -> Result<Self> {
        let content;
        parenthesized!(content in input);
        Ok(PropertyMeta { name: content.parse()? })
    }
}

impl Function {
    fn parse(sig: &Signature) -> Result<Self> {
        let name = sig.ident.clone();
        let mut inputs = sig.inputs.iter().peekable();

        let receivers = parse_receivers(&mut inputs);
        let mut parameters = Vec::new();
        let mut rest = None;

        let value = parse_quote!(vm::Value);
        let values = parse_quote!(&[vm::Value]);
        while let Some(&param) = inputs.peek() {
            match *param {
                FnArg::Typed(PatType { ref ty, .. }) if *ty == value =>
                    parameters.push(Parameter::Direct),

                FnArg::Typed(PatType { ref ty, .. }) if *ty == values => {
                    rest = Some(());
                    inputs.next();
                    break;
                }

                _ => parameters.push(Parameter::Convert),
            }
            inputs.next();
        }

        if let Some(param) = inputs.next() {
            return Err(Error::new(param.span(), "unexpected parameter"));
        }

        let result: Ident = parse_quote!(Result);
        let output = match sig.output {
            ReturnType::Default => Return::Value,
            ReturnType::Type(_, ref ty) => match **ty {
                Type::Path(ref ty) if ty.path.segments[0].ident == result => Return::Result,
                _ => Return::Value,
            },
        };

        Ok(Function { name, receivers, parameters, rest, output })
    }
}

impl Property {
    fn parse(sig: &Signature) -> Result<Self> {
        let name = sig.ident.clone();
        let mut inputs = sig.inputs.iter().peekable();

        let receivers = parse_receivers(&mut inputs);
        let mut entity = None;
        let mut index = None;
        let mut value = None;

        let entity_ty = parse_quote!(vm::Entity);
        let usize_ty = parse_quote!(usize);
        while let Some(&param) = inputs.peek() {
            match *param {
                FnArg::Typed(PatType { ref ty, .. }) if *ty == entity_ty =>
                    entity = Some(()),

                FnArg::Typed(PatType { ref ty, .. }) if *ty == usize_ty =>
                    index = Some(()),

                _ => break,
            }
            inputs.next();
        }

        let value_ty = parse_quote!(vm::Value);
        while let Some(&param) = inputs.peek() {
            match *param {
                FnArg::Typed(PatType { ref ty, .. }) if *ty == value_ty =>
                    value = Some(Parameter::Direct),

                FnArg::Typed(PatType { .. }) =>
                    value = Some(Parameter::Convert),

                _ => break,
            }
            inputs.next();
            break;
        }

        if let Some(param) = inputs.next() {
            return Err(Error::new(param.span(), "unexpected parameter"));
        }

        Ok(Property { name, receivers, entity, index, value })
    }
}

fn parse_receivers(inputs: &mut iter::Peekable<punctuated::Iter<'_, FnArg>>) -> Vec<Receiver> {
    let mut receivers = Vec::new();

    let self_ref = parse_quote!(&self);
    let self_mut = parse_quote!(&mut self);
    let world_ref = parse_quote!(&vm::World);
    let world_mut = parse_quote!(&mut vm::World);
    while let Some(&param) = inputs.peek() {
        match *param {
            _ if *param == self_ref || *param == self_mut =>
                receivers.push(Receiver::Self_),

            FnArg::Typed(PatType { ref ty, .. }) if *ty == world_ref || *ty == world_mut =>
                receivers.push(Receiver::World),

            _ => break,
        }
        inputs.next();
    }

    receivers
}

#[proc_macro_attribute]
pub fn bind(attr: TokenStream, input: TokenStream) -> TokenStream {
    // Parse the impl and collect method attributes.

    let trait_name: Ident = syn::parse_macro_input!(attr);
    let mut input: ItemImpl = syn::parse_macro_input!(input);
    let bindings = match ItemBindings::parse(&mut input) {
        Ok(bindings) => bindings,
        Err(errors) => {
            let errors: proc_macro2::TokenStream = errors.iter()
                .flat_map(Error::to_compile_error)
                .collect();
            return TokenStream::from(errors);
        }
    };

    // Generate the API glue trait.

    let self_ty = &input.self_ty;

    let api = bindings.functions.iter().map(|function| &function.name);
    let api_binding = api.clone();
    let api_arity = bindings.functions.iter().map(|function| function.parameters.len());
    let api_variadic = bindings.functions.iter().map(|function| function.rest.is_some());
    let api_receivers = bindings.functions.iter().map(|function| {
        let receivers = function.receivers.iter();
        quote! { #(#receivers,)* }
    });
    let api_arguments = bindings.functions.iter().map(|function| {
        let api_arguments = function.parameters.iter().enumerate().map(|(i, &param)| match param {
            Parameter::Direct => quote! { arguments[#i] },
            Parameter::Convert => quote! { arguments[#i].try_into().unwrap_or_default() },
        });
        quote! { #(#api_arguments,)* }
    });
    let api_rest = bindings.functions.iter().map(|function| {
        let arity = function.parameters.len();
        let rest = function.rest.iter().map(|()| quote! { &arguments[#arity..] });
        quote! { #(#rest,)* }
    });
    let api_try = bindings.functions.iter().map(|function| {
        match function.output {
            Return::Value => None,
            Return::Result => Some(quote! { ? }),
        }
    });

    let member = bindings.members.iter().map(|(name, _)| name);

    let getter = bindings.members.iter().map(|(_, member)| member.getter.as_ref());
    let get_option = getter.clone().map(|getter| getter.map_or_else(
        || quote! { None },
        |&Property { ref name, .. }| quote! { Some(Self::#name) },
    ));
    let get = getter.clone().flatten().map(|getter| &getter.name);
    let get_receivers = getter.clone().flatten().map(|getter| {
        let receivers = getter.receivers.iter();
        quote! { #(#receivers,)* }
    });
    let get_entity = getter.clone().flatten().map(|getter| {
        let entity = getter.entity.iter().map(|()| quote! { entity });
        quote! { #(#entity,)* }
    });
    let get_index = getter.clone().flatten().map(|getter| {
        let index = getter.index.iter().map(|()| quote! { index });
        quote! { #(#index,)* }
    });

    let setter = bindings.members.iter().map(|(_, member)| member.setter.as_ref());
    let set_option = setter.clone().map(|setter| setter.map_or_else(
        || quote! { None },
        |&Property { ref name, .. }| quote! { Some(Self::#name) },
    ));
    let set = setter.clone().flatten().map(|setter| &setter.name);
    let set_receivers = setter.clone().flatten().map(|setter| {
        let receivers = setter.receivers.iter();
        quote! { #(#receivers,)* }
    });
    let set_entity = setter.clone().flatten().map(|setter| {
        let entity = setter.entity.iter().map(|()| quote! { entity });
        quote! { #(#entity,)* }
    });
    let set_index = setter.clone().flatten().map(|setter| {
        let index = setter.index.iter().map(|()| quote! { index });
        quote! { #(#index,)* }
    });
    let set_value = setter.clone().flatten().map(|setter| {
        setter.value.map(|param| match param {
            Parameter::Direct => quote! { value },
            Parameter::Convert => quote! { value.try_into().unwrap_or_default() },
        })
    });

    let output = quote! {
        pub trait #trait_name {
            fn state(&self) -> (&#self_ty, &vm::World);
            fn state_mut(&mut self) -> (&mut #self_ty, &mut vm::World);

            fn register(items: &mut std::collections::HashMap<gml::symbol::Symbol, gml::Item<Self>>) where
                Self: Sized
            {
                #(items.insert(
                    gml::symbol::Symbol::intern(stringify!(#api_binding)),
                    gml::Item::Native(Self::#api_binding, #api_arity, #api_variadic),
                );)*

                #(items.insert(
                    gml::symbol::Symbol::intern(stringify!(#member)),
                    gml::Item::Member(#get_option, #set_option),
                );)*
            }

            #(fn #api(&mut self, arguments: &[vm::Value]) -> Result<vm::Value, vm::ErrorKind> {
                #![allow(unused_imports, unused)]
                use std::convert::TryInto;

                let (state, world) = self.state_mut();
                let ret = #self_ty::#api(#api_receivers #api_arguments #api_rest) #api_try;
                Ok(ret.into())
            })*

            #(fn #get(&self, entity: vm::Entity, index: usize) -> vm::Value {
                #![allow(unused)]

                let (state, world) = self.state();
                let value = #self_ty::#get(#get_receivers #get_entity #get_index);
                value.into()
            })*

            #(fn #set(&mut self, entity: vm::Entity, index: usize, value: vm::Value) {
                #![allow(unused_imports, unused)]
                use std::convert::TryInto;

                let (state, world) = self.state_mut();
                #self_ty::#set(#set_receivers #set_entity #set_index #set_value);
            })*
        }

        #input
    };
    output.into()
}

impl quote::ToTokens for Receiver {
    fn to_tokens(&self, tokens: &mut proc_macro2::TokenStream) {
        match *self {
            Receiver::Self_ => tokens.extend(quote! { state }),
            Receiver::World => tokens.extend(quote! { world }),
        }
    }
}
