// Copyright 2020 Bobby Powers. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

fn main() {
    let mut xmutil_build = cc::Build::new();
    xmutil_build
        .cpp(true)
        .include("./third_party/tinyxml2")
        .include("./third_party")
        .flag_if_supported("-std=c++14")
        .flag_if_supported("-Wunused-private-field")
        .flag_if_supported("-Wno-attributes")
        .flag_if_supported("-Wno-unused-variable")
        .flag_if_supported("-Wno-unused-but-set-variable")
        .flag_if_supported("-Wno-sign-compare")
        .flag_if_supported("-Wno-unused-local-typedefs")
        .flag_if_supported("-Wno-unused-parameter")
        .flag_if_supported("-Wno-unused-function")
        .flag_if_supported("-Wno-unknown-pragmas")
        .flag_if_supported("-Wno-parentheses")
        .define("NDEBUG", None)
        .file("./third_party/xmutil/Function/TableFunction.cpp")
        .file("./third_party/xmutil/Function/Level.cpp")
        .file("./third_party/xmutil/Function/State.cpp")
        .file("./third_party/xmutil/Function/Function.cpp")
        .file("./third_party/xmutil/Xmile/XMILEGenerator.cpp")
        .file("./third_party/xmutil/Vensim/VensimView.cpp")
        .file("./third_party/xmutil/Vensim/VensimParseFunctions.cpp")
        .file("./third_party/xmutil/Vensim/VYacc.tab.cpp")
        .file("./third_party/xmutil/Vensim/VensimLex.cpp")
        .file("./third_party/xmutil/Vensim/VensimParse.cpp")
        .file("./third_party/xmutil/Model.cpp")
        .file("./third_party/xmutil/Log.cpp")
        .file("./third_party/xmutil/Unicode.cpp")
        .file("./third_party/xmutil/ContextInfo.cpp")
        .file("./third_party/xmutil/XMUtil.cpp")
        .file("./third_party/xmutil/Symbol/Symbol.cpp")
        .file("./third_party/xmutil/Symbol/SymbolTableBase.cpp")
        .file("./third_party/xmutil/Symbol/SymbolList.cpp")
        .file("./third_party/xmutil/Symbol/Units.cpp")
        .file("./third_party/xmutil/Symbol/NotUsed_SymAllocList.cpp")
        .file("./third_party/xmutil/Symbol/SymbolNameSpace.cpp")
        .file("./third_party/xmutil/Symbol/SymbolListList.cpp")
        .file("./third_party/xmutil/Symbol/LeftHandSide.cpp")
        .file("./third_party/xmutil/Symbol/ExpressionList.cpp")
        .file("./third_party/xmutil/Symbol/Expression.cpp")
        .file("./third_party/xmutil/Symbol/Equation.cpp")
        .file("./third_party/xmutil/Symbol/Variable.cpp")
        .file("./third_party/xmutil/Symbol/UnitExpression.cpp");

    let mut tinyxml_build = cc::Build::new();
    tinyxml_build
        .cpp(true)
        .file("./third_party/tinyxml2/tinyxml2.cpp");

    let target = std::env::var("TARGET").unwrap();
    if target.starts_with("wasm") {
        // xmutil_build.cpp_set_stdlib("c++");
        xmutil_build.cpp_link_stdlib("c++-noexcept");
        // tinyxml_build.cpp_set_stdlib("c++");
        tinyxml_build.cpp_link_stdlib("c++-noexcept");
        println!(
            "cargo:rustc-link-search={}/src/emscripten/cache/wasm/",
            std::env::var("HOME").unwrap()
        );
        println!("cargo:rustc-link-lib=c");
    }

    xmutil_build.compile("xmutil-native");
    tinyxml_build.compile("tinyxml-native");

    cc::Build::new()
        .flag_if_supported("-Wno-parentheses")
        .flag_if_supported("-Wno-sign-compare")
        .file("third_party/libutf/rune.c")
        .file("third_party/libutf/runestrcat.c")
        .file("third_party/libutf/runestrchr.c")
        .file("third_party/libutf/runestrcmp.c")
        .file("third_party/libutf/runestrcpy.c")
        .file("third_party/libutf/runestrdup.c")
        .file("third_party/libutf/runestrecpy.c")
        .file("third_party/libutf/runestrlen.c")
        .file("third_party/libutf/runestrncat.c")
        .file("third_party/libutf/runestrncmp.c")
        .file("third_party/libutf/runestrncpy.c")
        .file("third_party/libutf/runestrrchr.c")
        .file("third_party/libutf/runestrstr.c")
        .file("third_party/libutf/runetype.c")
        .file("third_party/libutf/utfecpy.c")
        .file("third_party/libutf/utflen.c")
        .file("third_party/libutf/utfnlen.c")
        .file("third_party/libutf/utfrrune.c")
        .file("third_party/libutf/utfrune.c")
        .file("third_party/libutf/utfutf.c")
        .compile("libutf-native");

    println!("cargo:rerun-if-changed=build.rs");

    println!("cargo:rerun-if-changed=third_party/libutf/rune.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrcat.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrchr.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrcmp.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrcpy.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrdup.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrecpy.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrlen.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrncat.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrncmp.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrncpy.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrrchr.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runestrstr.c");
    println!("cargo:rerun-if-changed=third_party/libutf/runetype.c");
    println!("cargo:rerun-if-changed=third_party/libutf/utfecpy.c");
    println!("cargo:rerun-if-changed=third_party/libutf/utflen.c");
    println!("cargo:rerun-if-changed=third_party/libutf/utfnlen.c");
    println!("cargo:rerun-if-changed=third_party/libutf/utfrrune.c");
    println!("cargo:rerun-if-changed=third_party/libutf/utfrune.c");
    println!("cargo:rerun-if-changed=third_party/libutf/utfutf.c");
    println!("cargo:rerun-if-changed=third_party/tinyxml2/tinyxml2.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/ContextInfo.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/Function.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/Level.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/State.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/TableFunction.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Log.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Model.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Equation.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Expression.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/ExpressionList.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/LeftHandSide.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/NotUsed_SymAllocList.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Symbol.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolList.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolListList.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolNameSpace.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolTableBase.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/UnitExpression.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Units.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Variable.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Unicode.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimLex.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimParse.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimParseFunctions.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimView.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VYacc.tab.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Xmile/XMILEGenerator.cpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/XMUtil.cpp");

    println!("cargo:rerun-if-changed=third_party/libutf/lib9.h");
    println!("cargo:rerun-if-changed=third_party/libutf/plan9.h");
    println!("cargo:rerun-if-changed=third_party/libutf/utfdef.h");
    println!("cargo:rerun-if-changed=third_party/libutf/utf.h");
    println!("cargo:rerun-if-changed=third_party/tinyxml2/tinyxml2.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/ContextInfo.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/Function.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/Level.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/State.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Function/TableFunction.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Log.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Model.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Equation.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Expression.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/ExpressionList.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/LeftHandSide.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/NotUsed_SymAllocList.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Parse.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Symbol.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolList.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolListList.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolNameSpace.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/SymbolTableBase.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/UnitExpression.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Units.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Symbol/Variable.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Unicode.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimLex.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimParseFunctions.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimParse.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VensimView.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/Vensim/VYacc.tab.hpp");
    println!("cargo:rerun-if-changed=third_party/xmutil/Xmile/XMILEGenerator.h");
    println!("cargo:rerun-if-changed=third_party/xmutil/XMUtil.h");
}
