/// Generate nvim api bindings
///
/// This build script will run `nvim --api-info` to get api-metadata from nvim whose format is
/// msgpack, and then we unpack the data using rmpv_serde. You can have a look at
/// `assets/nvim-api-info.json5` for references.

use std::process::Command;
use std::fs::File;
use std::path::Path;
use std::io::Write;

use quote::{quote, format_ident};
use proc_macro2::TokenStream;

// This function is used to print message to console in build.rs script, it should be only used for
// debugging purpose. Note that only single line message is valid, message which contains newline
// will not work.
fn cargo_print<S: AsRef<str>>(msg: S) {
    let msg = msg.as_ref();
    println!("cargo:warning={} ==> {}", env!("CARGO_PKG_NAME"), msg);
}

// neovim's api metadata stuffs in rust representation
mod nvim {
    use std::collections::HashMap;
    use proc_macro2::TokenStream;
    use quote::{quote, format_ident, ToTokens};
    use serde::{Deserialize, Serialize};

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Version {
        pub major: u32,
        pub minor: u32,
        pub patch: u32,
        pub api_level: u32,
        pub api_compatible: u32,
        pub api_prerelease: bool,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Function {
        // true for ext type method, or else global method
        pub method: bool,
        pub name: String,
        pub since: u32,
        pub deprecated_since: Option<u32>,
        pub parameters: Vec<Vec<String>>,
        pub return_type: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct UiEvent {
        pub parameters: Vec<Vec<String>>,
        pub since: u32,
        pub name: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct ErrorType {
        pub id: u32,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Type {
        pub id: u32,
        pub prefix: String,
    }

    #[derive(Debug, Deserialize, Serialize)]
    pub struct Api {
        pub version: Version,
        pub functions: Vec<Function>,
        pub ui_events: Vec<UiEvent>,
        pub ui_options: Vec<String>,
        pub error_types: HashMap<String, ErrorType>,
        pub types: HashMap<String, Type>,
    }

    pub struct FunctionTokenStream {
        pub name: TokenStream,
        pub parameters: Vec<(TokenStream, TokenStream)>,
        pub return_type: TokenStream,
    }

    pub fn parse_vartype(typ: &str) -> TokenStream {
        let mut typemap = HashMap::new();
        typemap.insert("Boolean", "bool");
        typemap.insert("Array", "Vec<Value>");
        typemap.insert("Dictionary", "Vec<(Value, Value)>");
        typemap.insert("void", "()");
        typemap.insert("Float", "f64");
        typemap.insert("Integer", "i64");
        typemap.insert("Object", "Value");
        typemap.insert("String", "String");
        typemap.insert("Buffer", "Buffer");
        typemap.insert("Window", "Window");
        typemap.insert("Tabpage", "Tabpage");
        if let Some(type_name) = typemap.get(typ) {
            return syn::parse_str::<syn::Type>(type_name)
                .expect(format!("failed parse ident type: {typ}").as_str())
                .to_token_stream();
        }
        let items = typ.split(&['(', ')', ',']).collect::<Vec<&str>>();
        return parse_vartype(items[1]);
    }

    impl FunctionTokenStream {
        pub fn new(func: &Function) -> Self {
            let name = if let Some(_) = func.deprecated_since {
                format_ident!("_{}", func.name)
            } else {
                format_ident!("{}", func.name)
            };
            let name = quote!(#name);

            let mut parameters = vec![];
            for arg in func.parameters.iter() {
                let (vartype, varname) = (arg[0].clone(), arg[1].clone());
                let varname = format_ident!("arg_{}", varname);
                parameters.push((quote!(#varname), parse_vartype(vartype.as_str())));
            }

            let return_type = parse_vartype(func.return_type.as_str());

            Self {
                name,
                parameters,
                return_type,
            }
        }
    }

    impl Api {
        pub fn is_ext_function(&self, func: &Function) -> bool {
            for typ in self.types.values() {
                if func.name.starts_with(typ.prefix.as_str()) {
                    return true
                }
            }
            false
        }
    }

    impl Function {
        pub fn generate_trait_method_decl(&self) -> TokenStream {
            let func_stream = FunctionTokenStream::new(&self);
            let func_args = func_stream.parameters.iter().map(|(arg, typ)| {
                quote!{ #arg : #typ }
            });
            let func_name = func_stream.name;
            let func_ret = func_stream.return_type;
            if func_ret.is_empty() {
                if self.method && self.deprecated_since.is_none() {
                    quote! {
                        fn #func_name<R: Read + Send + 'static, W: Write + Send + 'static>(&self, client: &mut Client<R, W>, #(#func_args),*) -> Result<()>
                    }
                } else {
                    quote! {
                        fn #func_name(&mut self, #(#func_args),*) -> Result<()>
                    }
                }
            } else {
                if self.method && self.deprecated_since.is_none() {
                    quote! {
                        fn #func_name<R: Read + Send + 'static, W: Write + Send + 'static>(&self, client: &mut Client<R, W>, #(#func_args),*) -> Result<#func_ret>
                    }
                } else {
                    quote! {
                        fn #func_name(&mut self, #(#func_args),*) -> Result<#func_ret>
                    }
                }
            }
        }

        pub fn generate_method(&self) -> TokenStream {
            let func_stream = FunctionTokenStream::new(&self);
            let method_head = self.generate_trait_method_decl();
            let func_name = self.name.clone();
            let func_args = func_stream.parameters.iter().map(|(arg, _typ)| {
                let argvar = format_ident!("{}", arg.to_string());
                match arg.to_string().as_str() {
                    "arg_buffer" | "arg_window" | "arg_tabpage" => {
                        quote!(#argvar.data)
                    }
                    _ => {
                        quote!(#argvar.into())
                    }
                }
            });
            let return_value = match self.return_type.as_str() {
                "Buffer" => {
                    quote!(Ok(Buffer::new(r)))
                }
                "Window" => {
                    quote!(Ok(Window::new(r)))
                }
                "Tabpage" => {
                    quote!(Ok(Tabpage::new(r)))
                }
                _ => {
                    quote!(r.try_value_into())
                }
            };
            let caller = if self.method && self.deprecated_since.is_none() {
                format_ident!("{}", "client")
            } else {
                format_ident!("{}", "self")
            };
            quote! {
                #method_head {
                    let msgid = #caller.msgid;
                    #caller.msgid += 1;
                    let req = Message::Request {
                        msgid,
                        method: #func_name.to_owned(),
                        params: vec![#(#func_args),*],
                    };
                    let (sender, receiver) = mpsc::channel();
                    #caller.tasks.lock().unwrap().insert(msgid, sender);
                    let writer = &mut *#caller.writer.lock().unwrap();
                    req.write_to(writer).expect("Error sending message");
                    match receiver.recv() {
                        Ok(Ok(r)) => {
                            #return_value
                        }
                        Ok(Err(e)) => {
                            Err(Error::Dirty(format!("{e:?}")))
                        }
                        Err(e) => {
                            Err(Error::Dirty(format!("{e:?}")))
                        }
                    }
                }
            }
        }
    }
}

pub fn main() {
    let output = Command::new("nvim")
        .arg("--api-info")
        .output()
        .expect("failed to run command nvim --api-info");
    let apibuf = output.stdout.as_slice();
    let api = rmp_serde::from_slice::<nvim::Api>(&apibuf).unwrap();

    let mut code_stream = TokenStream::new();
    code_stream.extend(quote!{
        pub struct Window {
            data: Value,
        }

        impl Window {
            pub fn new(data: Value) -> Self {
                Self { data }
            }
        }

        pub struct Buffer {
            data: Value,
        }

        impl Buffer {
            pub fn new(data: Value) -> Self {
                Self { data }
            }
        }

        pub struct Tabpage {
            data: Value,
        }

        impl Tabpage {
            pub fn new(data: Value) -> Self {
                Self { data }
            }
        }

        pub trait TryValueFrom<T> {
            fn try_value_from(_: T) -> crate::Result<Self> where Self: Sized;
        }

        pub trait TryValueInto<T> {
            fn try_value_into(self) -> crate::Result<T>;
        }

        impl<T, U> TryValueInto<U> for T where U: TryValueFrom<T> {
            fn try_value_into(self) -> crate::Result<U> {
                U::try_value_from(self)
            }

        }

        impl TryValueFrom<crate::Value> for Vec<crate::Value> {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                let arr = value.as_array().ok_or(crate::Error::new("value is not an array"))?;
                return Ok(arr.to_owned().into_iter().collect());
            }
        }

        impl TryValueFrom<crate::Value> for i64 {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                value.as_i64().ok_or(crate::Error::new("value is not i64"))
            }
        }

        impl TryValueFrom<crate::Value> for () {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(())
            }
        }

        impl TryValueFrom<crate::Value> for String {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(value.as_str().ok_or(crate::Error::new("value is not a string"))?.to_owned())
            }
        }

        impl TryValueFrom<crate::Value> for bool {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(value.as_bool().ok_or(crate::Error::new("value is not bool"))?.to_owned())
            }
        }

        impl TryValueFrom<crate::Value> for rmpv::Value {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(value)
            }
        }

        impl TryValueFrom<crate::Value> for Vec<(rmpv::Value, rmpv::Value)> {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(value.as_map().ok_or(crate::Error::new("value is not a map"))?.to_vec())
            }
        }

        impl TryValueFrom<crate::Value> for Buffer {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(Buffer::new(value))
            }
        }

        impl TryValueFrom<crate::Value> for Window {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(Window::new(value))
            }
        }

        impl TryValueFrom<crate::Value> for Tabpage {
            fn try_value_from(value: crate::Value) -> crate::Result<Self> {
                Ok(Tabpage::new(value))
            }
        }
    });

    let neovim_api_trait = {
        let mut global_api_trait_methods = TokenStream::new();
        for func in api.functions.iter() {
            if api.is_ext_function(&func) || func.deprecated_since.is_some() {
                continue;
            }
            let method_decl = func.generate_trait_method_decl();
            global_api_trait_methods.extend(quote::quote!{ #method_decl; });
        }
        quote! {
            pub trait NeovimApi {
                #global_api_trait_methods
            }
        }
    };
    code_stream.extend(neovim_api_trait);

    let neovim_api_trait_impl = {
        let mut global_api_trait_methods_impl = TokenStream::new();
        for func in api.functions.iter() {
            if api.is_ext_function(&func) || func.deprecated_since.is_some() {
                continue;
            }
            global_api_trait_methods_impl.extend(func.generate_method());
        }
        quote! {
            impl<R: Read + Send + 'static, W: Write + Send + 'static> NeovimApi for Client<R, W> {
                #global_api_trait_methods_impl
            }
        }
    };
    code_stream.extend(neovim_api_trait_impl);

    let extmap = vec![("Buffer", "nvim_buf_"), ("Window", "nvim_win_"), ("Tabpage", "nvim_tabpage_")];
    for (typ, prefix) in extmap {
        let neovim_ext_api = {
            let mut stream = TokenStream::new();
            for func in api.functions.iter() {
                // TODO(2022-05-19): support function such as `nvim_buf_call([["Buffer", "buffer"], ["LuaRef", "fun"]])` and deprecated api
                if func.name == "nvim_buf_call" || func.name == "nvim_win_call" || func.deprecated_since.is_some() {
                    continue;
                }
                if api.is_ext_function(&func) && func.name.starts_with(prefix) {
                    stream.extend(func.generate_method());
                }
            }
            stream
        };
        let neovim_ext_api_impl = {
            let exttype = format_ident!("{}", typ);
            quote! {
                impl #exttype {
                    #neovim_ext_api
                }
            }
        };
        code_stream.extend(neovim_ext_api_impl);
    }

    let ast: syn::File = syn::parse2(code_stream).expect("not a valid tokenstream");
    let code = prettyplease::unparse(&ast);
    let mut buf = String::new();
    buf.push_str(&code);

    let outdir = std::env::var("OUT_DIR").unwrap();
    let outfile = Path::new(outdir.as_str()).join("nvim_api.rs");
    let mut f = File::create(outfile.as_path()).expect("failed to create nvim_api.rs");
    f.write_all(buf.as_bytes()).expect("failed to write nvim_api.rs");
    cargo_print(format!("nvim_api.rs: {}", outfile.display()));
}
