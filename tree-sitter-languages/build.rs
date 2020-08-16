use std::env;
use std::error::Error;
use std::fs;
use std::path::PathBuf;
use std::process::Command;

use quote::{format_ident, quote};

fn main() -> Result<(), Box<dyn Error>> {
    let mut functions = vec![];
    let mut tests = vec![];

    for entry in fs::read_dir("vendor")? {
        let repo_path = entry?.path();
        let language = repo_path
            .file_name()
            .expect("tree-sitter language repo paths must have a file name")
            .to_str()
            .expect("tree-sitter language repo paths must be UTF-8")
            .trim_start_matches("tree-sitter-");

        let src = repo_path.join("src");
        let vendor_dir = format!("tree-sitter-{}", language);
        cc::Build::new()
            .warnings(false)
            .include(&src)
            .file(src.join("parser.c"))
            .file(src.join("scanner.c"))
            .compile(&vendor_dir);

        let language_ident = format_ident!("{}", language);
        let tree_sitter_function = format_ident!("tree_sitter_{}", language);
        let highlight_query_path = PathBuf::from(env::var("CARGO_MANIFEST_DIR")?)
            .join("vendor")
            .join(vendor_dir)
            .join("queries/highlights.scm");
        let highlight_query_path = highlight_query_path
            .to_str()
            .expect("expected path to be UTF-8");
        functions.push(quote! {
            pub fn #language_ident() -> (Language, Query) {
                extern "C" {
                    fn #tree_sitter_function() -> tree_sitter::Language;
                }

                let language = unsafe { #tree_sitter_function() };
                let query = Query::new(
                    language,
                    include_str!(#highlight_query_path),
                ).expect("unable to parse highlight query");
                (language, query)
            }
        });

        tests.push(quote! {
            #[test]
            fn #language_ident() {
                println!("{:?}", super::#language_ident());
            }
        });
    }

    let tokens = quote! {
        use tree_sitter::{Language, Query};

        #(#functions)*

        #[cfg(test)]
        mod tests {
            #(#tests)*
        }
    };

    let out_dir = PathBuf::from(env::var("OUT_DIR")?);
    let out_file = out_dir.join("generated.rs");
    fs::write(&out_file, tokens.to_string())?;

    // Attempt to format the output for better human readability.
    let _ = Command::new("rustfmt")
        .args(&["--emit", "files"])
        .arg(out_file)
        .status();

    Ok(())
}
