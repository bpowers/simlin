extern crate lalrpop;

fn main() {
  lalrpop::process_root().unwrap();
  prost_build::compile_protos(&["ast.proto"],
                              &["src"]).unwrap();
}
