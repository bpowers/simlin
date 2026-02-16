// Copyright 2026 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

#[cfg(test)]
mod tests {
    use crate::open_vensim_xmutil;
    use std::fs;

    #[test]
    fn test_open_vensim_xmutil_sir() {
        let mdl_path = "../libsimlin/testdata/SIR.mdl";
        let mdl_content = fs::read_to_string(mdl_path).unwrap();

        let project = open_vensim_xmutil(&mdl_content)
            .expect("open_vensim_xmutil should successfully parse SIR.mdl");

        // Validate the project has expected structure
        assert!(
            !project.models.is_empty(),
            "Project should have at least one model"
        );

        // Check that the main model exists
        let main_model = project
            .models
            .iter()
            .find(|m| m.name == "main")
            .expect("Project should have a main model");

        // Validate the model has variables
        assert!(
            !main_model.variables.is_empty(),
            "Main model should have variables"
        );
    }
}
