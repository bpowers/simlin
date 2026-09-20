// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! What the XMILE reader reports it does not keep
//! ([`crate::xmile::project_from_reader_with_warnings`]).

use crate::xmile::{project_from_reader, project_from_reader_with_warnings};

/// A Stella file holding one of each kind of thing the reader does not keep,
/// each called what Stella shows it by (a name, a title, a label, the
/// variable it sets or plots, its text), and written the ways that most
/// test a reader that skips them: a text box with text on both sides of a
/// child element, an entity without a name, and story mode twice.
const FILE: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" xmlns:isee="http://iseesystems.com/XMILE">
    <header>
        <name>losses</name>
        <vendor>isee systems, inc.</vendor>
        <product version="3.0" lang="en">Stella Architect</product>
    </header>
    <sim_specs method="Euler" time_units="Months">
        <start>1</start>
        <stop>13</stop>
        <dt>0.25</dt>
    </sim_specs>
    <isee:prefs show_module_prefix="true" layer="model"/>
    <model_units/>
    <model>
        <variables>
            <stock name="Population">
                <eqn>100</eqn>
                <inflow>births</inflow>
            </stock>
            <flow name="births">
                <eqn>Population * birth_rate</eqn>
            </flow>
            <aux name="birth rate">
                <eqn>0.1</eqn>
            </aux>
            <gf name="effect table">
                <xscale min="0" max="1"/>
                <ypts>0,1</ypts>
            </gf>
            <group name="Sector 1">
                <entity name="Population"/>
                <entity name="births"/>
            </group>
            <isee:dependencies>
                <var name="births">
                    <in>Population</in>
                </var>
            </isee:dependencies>
        </variables>
        <views>
            <style color="black"/>
            <view type="stock_flow" page_width="800" page_height="600">
                <style color="black"/>
                <stock x="200" y="200" name="Population"/>
                <flow x="120" y="200" name="births">
                    <pts>
                        <pt x="50" y="200"/>
                        <pt x="177.5" y="200"/>
                    </pts>
                </flow>
                <aux x="100" y="300" name="birth rate"/>
                <stacked_container uid="1" x="400" y="100" width="300" height="200">
                    <graph type="time_series" title=" " isee:page_title="Population over time">
                        <plot index="0"><entity name="Population"/></plot>
                    </graph>
                </stacked_container>
                <stacked_container uid="8" x="400" y="300">
                    <graph type="bar"><plot index="0"><entity name="births"/></plot></graph>
                </stacked_container>
                <text_box uid="2" x="400" y="400" width="200" height="50">Read me first</text_box>
            </view>
            <view type="interface" page_width="800" page_height="600">
                <style/>
                <slider uid="3" x="10" y="10" width="200" height="50" min="0" max="1" title="Birth Rate">
                    <entity name="birth_rate"/>
                    <reset_to after="never">0</reset_to>
                </slider>
                <slider uid="4" x="10" y="80" width="200" height="50" min="0" max="1">
                    <entity name="Population"/>
                </slider>
                <slider uid="5" x="10" y="150" width="200" height="50"><entity/></slider>
                <text_box uid="6" x="300" y="10" width="200" height="50">Text <b>around</b> a child</text_box>
                <button uid="7" x="300" y="100" width="80" height="30" label="Run"/>
                <isee:annotation uid="9" x="250" y="10" launcher_label="?">
                    <popup x="260" y="20"><text>Why the birth rate matters</text></popup>
                </isee:annotation>
            </view>
            <view type="interface" page_width="800" page_height="600">
                <style/>
            </view>
            <isee:templates>
                <view type="interface"/>
            </isee:templates>
            <isee:stories>
                <isee:story name="Introduction"/>
                <isee:story name="Policy"><isee:chapter>text</isee:chapter></isee:story>
            </isee:stories>
            <isee:stories/>
        </views>
    </model>
</xmile>
"#;

#[test]
fn a_file_reports_each_kind_of_element_it_holds_but_the_project_does_not() {
    let (project, warnings) =
        project_from_reader_with_warnings(&mut FILE.as_bytes()).expect("the file opens");
    let messages: Vec<&str> = warnings.iter().map(|w| w.message.as_str()).collect();
    assert_eq!(
        messages,
        [
            "1 graphical function in the model is not kept: 'effect table'",
            "1 group's member list in the model is not kept: 'Sector 1'",
            "2 graphs and tables on the diagram are not kept: 'Population over time' and 'births'",
            "1 text box on the diagram is not kept: 'Read me first'",
            "3 sliders on interface page 1 are not kept, such as 'Birth Rate' and 'Population'",
            "1 text box on interface page 1 is not kept: 'Text a child'",
            "1 button on interface page 1 is not kept: 'Run'",
            "1 annotation on interface page 1 is not kept: 'Why the birth rate matters'",
            "2 stories in story mode are not kept: 'Introduction' and 'Policy'",
        ]
    );

    // Reporting reads the same project the plain open does, and the
    // elements it skips cost the model nothing it reads.
    assert!(project == project_from_reader(&mut FILE.as_bytes()).unwrap());
    let model = project.get_model("main").expect("one model");
    let mut idents: Vec<&str> = model.variables.iter().map(|v| v.get_ident()).collect();
    idents.sort_unstable();
    assert_eq!(idents, ["Population", "birth rate", "births"]);
}

#[test]
fn a_file_of_only_what_the_project_keeps_reports_nothing() {
    let file = r#"<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0">
    <header><name>kept</name><vendor>test</vendor><product version="1">test</product></header>
    <sim_specs><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
    <model>
        <variables><aux name="a"><eqn>1</eqn></aux></variables>
        <views>
            <view><style/><aux x="10" y="10" name="a"/></view>
            <view type="interface"><style/></view>
        </views>
    </model>
</xmile>"#;
    let (_, warnings) = project_from_reader_with_warnings(&mut file.as_bytes()).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
}

/// A file of one auxiliary, drawn, with `variables` added among its
/// variables and `views` among its views after the diagram, whose own
/// objects `diagram` adds to.
fn file_with(variables: &str, diagram: &str, views: &str) -> String {
    format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<xmile version="1.0" xmlns="http://docs.oasis-open.org/xmile/ns/XMILE/v1.0" xmlns:isee="http://iseesystems.com/XMILE">
<header><name>t</name><vendor>isee systems, inc.</vendor><product version="3.0" lang="en">Stella Architect</product></header>
<sim_specs method="Euler" time_units="Months"><start>0</start><stop>10</stop><dt>1</dt></sim_specs>
<model>
<variables><aux name="a"><eqn>1</eqn></aux>{variables}</variables>
<views><view><aux x="1" y="1" name="a"/>{diagram}</view>{views}</views>
</model></xmile>"#
    )
}

/// The reporting open of `file` reads what the plain open reads, and its
/// warnings.
fn report(file: &str) -> Vec<String> {
    let plain = project_from_reader(&mut file.as_bytes()).expect("the plain open");
    let (project, warnings) =
        project_from_reader_with_warnings(&mut file.as_bytes()).expect("the reporting open");
    assert!(project == plain, "the report changed the project of {file}");
    warnings.into_iter().map(|w| w.message).collect()
}

#[test]
fn ill_formed_content_the_read_skips_opens_and_is_reported_without_a_label() {
    // Each file opens without the report, so each opens with it, and each
    // unread element is counted. What does not unescape names nothing.
    let text_box = "1 text box on the diagram is not kept";
    let cases: [(&str, &str, &str, &[&str]); 13] = [
        (
            "",
            "<text_box uid=\"2\">caf&nbsp;e</text_box>",
            "",
            &[text_box],
        ),
        (
            "",
            "<text_box uid=\"2\" label=\"a &bogus; b\"/>",
            "",
            &[text_box],
        ),
        (
            "",
            "",
            "<isee:stories><isee:story name=\"x &bogus;\"/></isee:stories>",
            &["1 story in story mode is not kept"],
        ),
        (
            "<gf name=\"t &bogus;\"><xscale min=\"0\" max=\"1\"/><ypts>0,1</ypts></gf>",
            "",
            "",
            &["1 graphical function in the model is not kept"],
        ),
        ("", "<text_box>&#0;</text_box>", "", &[text_box]),
        ("", "<text_box>&#xD800;</text_box>", "", &[text_box]),
        ("", "<text_box title=\"a & b\"/>", "", &[text_box]),
        (
            "",
            "<stacked_container><graph title=\"&nbsp;\"/></stacked_container>",
            "",
            &["1 graph or table on the diagram is not kept"],
        ),
        (
            "",
            "<text_box><entity name=\"&nbsp;\"/></text_box>",
            "",
            &[text_box],
        ),
        (
            "",
            "<text_box><other>&nbsp;</other></text_box>",
            "",
            &[text_box],
        ),
        (
            "",
            "",
            "<view type=\"interface\"><text_box>&nbsp;</text_box></view>",
            &["1 text box on the interface page is not kept"],
        ),
        // What the report leaves out is not looked at.
        ("", "<style>&nbsp;</style>", "", &[]),
        (
            "<isee:dependencies><var name=\"&nbsp;\"/></isee:dependencies>",
            "",
            "",
            &[],
        ),
    ];
    for (variables, diagram, views, expected) in cases {
        let file = file_with(variables, diagram, views);
        assert_eq!(report(&file), expected, "{variables}{diagram}{views}");
    }

    // A file the read refuses, the report refuses the same way.
    let file = file_with("", "<text_box>a & b</text_box>", "");
    let plain = project_from_reader(&mut file.as_bytes()).err();
    let reporting = project_from_reader_with_warnings(&mut file.as_bytes()).err();
    assert!(plain.is_some());
    assert_eq!(format!("{plain:?}"), format!("{reporting:?}"));

    // The same elements, well formed, are named.
    let file = file_with(
        "",
        "<text_box>caf&#233; &amp; bar</text_box><text_box title=\"a &amp; b\"/>",
        "",
    );
    assert_eq!(
        report(&file),
        ["2 text boxes on the diagram are not kept: 'café & bar' and 'a & b'"]
    );
}

#[test]
fn an_element_nested_ten_thousand_deep_opens_on_a_small_stack() {
    // A secondary thread's usual stack. The label is read no deeper than a
    // Stella graph's plotted variable, so the title at the bottom is not
    // reached.
    const STACK: usize = 512 * 1024;
    const DEPTH: usize = 10_000;
    for tag in ["graph", "zz"] {
        let nested = format!(
            "<text_box>{}<{tag} title=\"deep\"/>{}</text_box>",
            format!("<{tag}>").repeat(DEPTH),
            format!("</{tag}>").repeat(DEPTH)
        );
        let file = file_with("", &nested, "");
        let warnings = std::thread::Builder::new()
            .stack_size(STACK)
            .spawn(move || report(&file))
            .expect("a thread")
            .join()
            .expect("the open returns");
        assert_eq!(warnings, ["1 text box on the diagram is not kept"], "{tag}");
    }
}

#[test]
fn a_file_of_many_pages_and_many_kinds_reports_each_once() {
    // Twenty thousand unnamed interface pages, each numbered among its
    // kind, and as many distinct kinds of element on the diagram.
    const MANY: usize = 20_000;
    let pages = "<view type=\"interface\"><text_box>t</text_box></view>".repeat(MANY);
    let kinds: String = (0..MANY).map(|i| format!("<t{i}>x</t{i}>")).collect();
    let warnings = report(&file_with("", &kinds, &pages));
    assert_eq!(warnings.len(), 2 * MANY);
    assert_eq!(warnings[0], "1 t0 on the diagram is not kept: 'x'");
    assert_eq!(
        warnings[MANY - 1],
        format!("1 t{} on the diagram is not kept: 'x'", MANY - 1)
    );
    assert_eq!(
        warnings[MANY],
        "1 text box on interface page 1 is not kept: 't'"
    );
    assert_eq!(
        warnings[2 * MANY - 1],
        format!("1 text box on interface page {MANY} is not kept: 't'")
    );
}
