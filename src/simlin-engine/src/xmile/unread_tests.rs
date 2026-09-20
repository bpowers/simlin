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
