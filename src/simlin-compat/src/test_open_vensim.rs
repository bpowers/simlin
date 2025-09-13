#[cfg(test)]
mod tests {
    use crate::open_vensim;
    use std::fs;
    use std::io::BufReader;

    #[test]
    fn test_open_vensim_sir() {
        let mdl_path = "../libsimlin/testdata/SIR.mdl";
        let mdl_content = fs::read_to_string(mdl_path).unwrap();

        let mut mdl_reader = BufReader::new(mdl_content.as_bytes());
        let project =
            open_vensim(&mut mdl_reader).expect("open_vensim should successfully parse SIR.mdl");

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
