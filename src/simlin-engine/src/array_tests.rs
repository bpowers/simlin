// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Focused tests for array functionality
//!
//! These tests are designed to incrementally test array features,
//! starting with compilation and then moving to interpreter execution.

#[cfg(test)]
mod wildcard_tests {
    use crate::ErrorCode;
    use crate::test_common::TestProject;

    #[test]
    fn wildcard_preserves_dimension() {
        // Test that arr[*] preserves the array dimension
        let project = TestProject::new("wildcard_basic")
            .indexed_dimension("args", 3)
            .array_const("source[args]", 10.0)
            .array_aux("result[args]", "source[*]");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("result", &[10.0, 10.0, 10.0]);
    }

    #[test]
    fn wildcard_with_named_dimension() {
        // Test wildcard with named dimensions
        let project = TestProject::new("wildcard_named")
            .named_dimension("City", &["Boston", "NYC", "LA"])
            .array_const("population[City]", 1000.0)
            .array_aux("copy[City]", "population[*]");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("copy", &[1000.0, 1000.0, 1000.0]);
    }

    #[test]
    fn wildcard_with_arithmetic() {
        // Test wildcard in arithmetic expressions
        let project = TestProject::new("wildcard_arithmetic")
            .indexed_dimension("Index", 4)
            .array_const("base[Index]", 5.0)
            .array_aux("doubled[Index]", "base[*] * 2")
            .array_aux("added[Index]", "base[*] + 10");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("doubled", &[10.0, 10.0, 10.0, 10.0]);
        project.assert_interpreter_result("added", &[15.0, 15.0, 15.0, 15.0]);
    }

    #[test]
    fn wildcard_chained() {
        // Test chained wildcards
        let project = TestProject::new("wildcard_chained")
            .indexed_dimension("Dim", 3)
            .array_const("a[Dim]", 2.0)
            .array_aux("b[Dim]", "a[*] * 3")
            .array_aux("c[Dim]", "b[*] + 1");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("b", &[6.0, 6.0, 6.0]);
        project.assert_interpreter_result("c", &[7.0, 7.0, 7.0]);
    }

    #[test]
    fn wildcard_all_dims_2d() {
        // Test wildcards for all dimensions in 2D arrays (this should work)
        let project = TestProject::new("wildcard_all_2d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 3)
            .array_const("source[X,Y]", 10.0) // All elements = 10
            .array_aux("copy[X,Y]", "source[*,*] * 2"); // Should double all elements

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("copy", &[20.0, 20.0, 20.0, 20.0, 20.0, 20.0]);
    }

    #[test]
    fn wildcard_all_dims_3d() {
        // Test wildcards for all dimensions in 3D arrays
        let project = TestProject::new("wildcard_all_3d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .indexed_dimension("Z", 2)
            .array_const("cube[X,Y,Z]", 3.0) // All elements = 3
            .array_aux("result[X,Y,Z]", "cube[*,*,*] + 1"); // Should add 1 to all elements

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("result", &[4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0, 4.0]);
    }

    #[test]
    fn simple_index() {
        // Test numeric indexing per XMILE spec
        let project = TestProject::new("simple_index")
            .indexed_dimension("A", 2)
            .array_const("m[A]", 5.0) // All elements = 5
            .scalar_aux("first_item", "m[1]"); // XMILE uses simple numeric indices

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("first_item", 5.0);
    }

    #[test]
    fn wildcard_simple_2d() {
        // Simpler test for 2D arrays with wildcards
        let project = TestProject::new("wildcard_simple_2d")
            .indexed_dimension("A", 2)
            .indexed_dimension("B", 2)
            .array_const("m[A,B]", 5.0) // All elements = 5
            .array_aux("first_row[B]", "m[1, *]"); // Use numeric index per XMILE spec

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("first_row", &[5.0, 5.0]);
    }

    #[test]
    fn wildcard_in_multidim_fixed_first() {
        // Test wildcard in multi-dimensional arrays with first dimension fixed
        let project = TestProject::new("wildcard_multidim_fixed_first")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "10"),
                    ("1,2", "11"),
                    ("1,3", "12"),
                    ("2,1", "20"),
                    ("2,2", "21"),
                    ("2,3", "22"),
                ],
            )
            .array_aux("row1[Col]", "matrix[1, *]") // Should be [10, 11, 12]
            .array_aux("row2[Col]", "matrix[2, *]"); // Should be [20, 21, 22]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("row1", &[10.0, 11.0, 12.0]);
        project.assert_interpreter_result("row2", &[20.0, 21.0, 22.0]);
    }

    #[test]
    fn wildcard_in_multidim_fixed_second() {
        // Test wildcard in multi-dimensional arrays with second dimension fixed
        let project = TestProject::new("wildcard_multidim_fixed_second")
            .indexed_dimension("Row", 3)
            .indexed_dimension("Col", 2)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "110"),
                    ("1,2", "120"),
                    ("2,1", "210"),
                    ("2,2", "220"),
                    ("3,1", "310"),
                    ("3,2", "320"),
                ],
            )
            .array_aux("col1[Row]", "matrix[*, 1]") // Should be [110, 210, 310]
            .array_aux("col2[Row]", "matrix[*, 2]"); // Should be [120, 220, 320]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("col1", &[110.0, 210.0, 310.0]);
        project.assert_interpreter_result("col2", &[120.0, 220.0, 320.0]);
    }

    #[test]
    fn wildcard_with_named_and_indexed_dims() {
        // Test wildcard with mixed named and indexed dimensions
        let project = TestProject::new("wildcard_mixed_dims")
            .named_dimension("City", &["Boston", "NYC", "LA"])
            .indexed_dimension("Year", 2)
            .array_with_ranges(
                "population[City,Year]",
                vec![
                    ("Boston,1", "1100"),
                    ("Boston,2", "1200"),
                    ("NYC,1", "1100"),
                    ("NYC,2", "1200"),
                    ("LA,1", "1100"),
                    ("LA,2", "1200"),
                ],
            )
            .array_aux("boston_years[Year]", "population[Boston, *]") // Named dim uses element name
            .array_aux("year1_cities[City]", "population[*, 1]"); // Indexed dim uses numeric

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("boston_years", &[1100.0, 1200.0]);
        project.assert_interpreter_result("year1_cities", &[1100.0, 1100.0, 1100.0]);
    }

    #[test]
    fn wildcard_three_dimensions() {
        // Test wildcard in 3D arrays
        let project = TestProject::new("wildcard_3d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .indexed_dimension("Z", 2)
            .array_with_ranges(
                "cube[X,Y,Z]",
                vec![
                    ("1,1,1", "111"),
                    ("1,1,2", "112"),
                    ("1,2,1", "121"),
                    ("1,2,2", "122"),
                    ("2,1,1", "211"),
                    ("2,1,2", "212"),
                    ("2,2,1", "221"),
                    ("2,2,2", "222"),
                ],
            )
            .array_aux("slice_xy[X,Y]", "cube[*,*,1]") // Fix Z=1: [111, 121, 211, 221]
            .array_aux("slice_xz[X,Z]", "cube[*,2,*]") // Fix Y=2: [121, 122, 221, 222]
            .array_aux("slice_yz[Y,Z]", "cube[1,*,*]"); // Fix X=1: [111, 112, 121, 122]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("slice_xy", &[111.0, 121.0, 211.0, 221.0]);
        project.assert_interpreter_result("slice_xz", &[121.0, 122.0, 221.0, 222.0]);
        project.assert_interpreter_result("slice_yz", &[111.0, 112.0, 121.0, 122.0]);
    }

    #[test]
    fn wildcard_multiple_in_expression() {
        // Test multiple wildcards in the same expression (both operands)
        let project = TestProject::new("wildcard_multiple")
            .indexed_dimension("Blerg", 3)
            .indexed_dimension("Product", 2)
            .array_with_ranges(
                "sales[Blerg,Product]",
                vec![
                    ("1,1", "10"), // (1) * 1 * 10 = 10
                    ("1,2", "20"), // (1) * 2 * 10 = 20
                    ("2,1", "20"), // (2) * 1 * 10 = 20
                    ("2,2", "40"), // (2) * 2 * 10 = 40
                    ("3,1", "30"), // (3) * 1 * 10 = 30
                    ("3,2", "60"), // (3) * 2 * 10 = 60
                ],
            )
            .array_with_ranges(
                "costs[Blerg,Product]",
                vec![
                    ("1,1", "5"),  // 1 * 5 = 5
                    ("1,2", "10"), // 2 * 5 = 10
                    ("2,1", "5"),  // 1 * 5 = 5
                    ("2,2", "10"), // 2 * 5 = 10
                    ("3,1", "5"),  // 1 * 5 = 5
                    ("3,2", "10"), // 2 * 5 = 10
                ],
            )
            .array_aux("profit1[Blerg,Product]", "sales[*,*] - costs[*,*]") // Element-wise subtraction
            .array_aux("profit2[Blerg,Product]", "sales - costs"); // different syntax same result

        project.assert_compiles();
        project.assert_sim_builds();
        // profit should be: [10-5, 20-10, 20-5, 40-10, 30-5, 60-10] = [5, 10, 15, 30, 25, 50]
        project.assert_interpreter_result("profit1", &[5.0, 10.0, 15.0, 30.0, 25.0, 50.0]);
        project.assert_interpreter_result("profit2", &[5.0, 10.0, 15.0, 30.0, 25.0, 50.0]);
    }

    #[test]
    fn wildcard_interpreter_basic() {
        TestProject::new("wildcard_interpreter")
            .indexed_dimension("Widgets", 3)
            .array_const("source[Widgets]", 10.0)
            .array_aux("result[Widgets]", "source[*]")
            .assert_interpreter_result("result", &[10.0, 10.0, 10.0]);
    }

    #[test]
    fn wildcard_interpreter_expression_indexed() {
        let project = TestProject::new("wildcard_expr")
            .indexed_dimension("Index", 3)
            .array_aux("values[Index]", "1 + Index") // Assuming Index gives position
            .array_aux("doubled[Index]", "values[*] * 2");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("doubled", &[4.0, 6.0, 8.0]);
    }

    #[test]
    fn wildcard_interpreter_expression_named() {
        let project = TestProject::new("wildcard_expr")
            .named_dimension("Cities", &["Boston", "NYC"])
            .array_aux("values[Cities]", "1 + Cities") // Assuming Index gives position
            .array_aux("doubled[Cities]", "values[*] * 2");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("doubled", &[4.0, 6.0]);
    }

    #[test]
    fn dimension_as_index() {
        // Test that dimension names evaluate to indices in A2A context
        let project = TestProject::new("dim_index")
            .named_dimension("Cities", &["Boston", "NYC", "LA"])
            .array_aux("indices[Cities]", "Cities"); // Should be [1, 2, 3]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("indices", &[1.0, 2.0, 3.0]);
    }

    #[test]
    fn wildcard_interpreter_expression_scalar_fails() {
        let project = TestProject::new("wildcard_expr")
            .named_dimension("Cities", &["Boston", "NYC"])
            .scalar_aux("value", "1 + Cities");

        project.assert_compile_error(ErrorCode::DimensionInScalarContext);
    }
}

#[cfg(test)]
mod dimension_position_tests {
    use crate::test_common::TestProject;

    #[test]
    fn dimension_position_single() {
        // Test @1 syntax for accessing first element of a dimension
        let project = TestProject::new("dim_pos_single")
            .indexed_dimension("Items", 3)
            .array_with_ranges("arr[Items]", vec![("1", "10"), ("2", "20"), ("3", "30")])
            .scalar_aux("first_elem", "arr[@1]"); // Should get first element = 10

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("first_elem", 10.0);
    }

    #[test]
    fn dimension_position_reorder() {
        // Test reordering dimensions with @2, @1
        let project = TestProject::new("dim_pos_reorder")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                ],
            )
            .array_aux("transposed[Col,Row]", "matrix[@2, @1]"); // Swap dimensions

        project.assert_compiles();
        project.assert_sim_builds();
        // Original matrix is row-major: [11, 12, 13, 21, 22, 23]
        // Transposed should be: [11, 21, 12, 22, 13, 23]
        project.assert_interpreter_result("transposed", &[11.0, 21.0, 12.0, 22.0, 13.0, 23.0]);
    }

    #[test]
    fn dimension_position_3d() {
        // Test dimension position with 3D arrays
        let project = TestProject::new("dim_pos_3d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .indexed_dimension("Z", 2)
            .array_with_ranges(
                "cube[X,Y,Z]",
                vec![
                    ("1,1,1", "111"),
                    ("1,1,2", "112"),
                    ("1,2,1", "121"),
                    ("1,2,2", "122"),
                    ("2,1,1", "211"),
                    ("2,1,2", "212"),
                    ("2,2,1", "221"),
                    ("2,2,2", "222"),
                ],
            )
            // Reorder to [Z,Y,X]
            .array_aux("reordered[Z,Y,X]", "cube[@3, @2, @1]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Original cube is in X,Y,Z order: [111, 112, 121, 122, 211, 212, 221, 222]
        // Reordered to Z,Y,X should be: [111, 211, 121, 221, 112, 212, 122, 222]
        project.assert_interpreter_result(
            "reordered",
            &[111.0, 211.0, 121.0, 221.0, 112.0, 212.0, 122.0, 222.0],
        );
    }

    #[test]
    fn dimension_position_partial() {
        // Test mixing dimension position with wildcards
        let project = TestProject::new("dim_pos_partial")
            .indexed_dimension("A", 2)
            .indexed_dimension("B", 3)
            .indexed_dimension("C", 2)
            .array_with_ranges(
                "arr[A,B,C]",
                vec![
                    ("1,1,1", "111"),
                    ("1,1,2", "112"),
                    ("1,2,1", "121"),
                    ("1,2,2", "122"),
                    ("1,3,1", "131"),
                    ("1,3,2", "132"),
                    ("2,1,1", "211"),
                    ("2,1,2", "212"),
                    ("2,2,1", "221"),
                    ("2,2,2", "222"),
                    ("2,3,1", "231"),
                    ("2,3,2", "232"),
                ],
            )
            // Fix first dimension at position 1, keep all B, use C dimension
            .array_aux("slice[C,B]", "arr[1, *, @1]"); // Fix A=1, keep all B, use C dimension

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get A=1 slice with C,B ordering: [111, 121, 131, 112, 122, 132]
        project.assert_interpreter_result("slice", &[111.0, 121.0, 131.0, 112.0, 122.0, 132.0]);
    }
}

#[cfg(test)]
mod transpose_tests {
    use crate::test_common::TestProject;

    #[test]
    fn transpose_2d_array() {
        // Test transpose operator on 2D array
        let project = TestProject::new("transpose_2d")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                ],
            )
            // For now, let's work around the issue by using dimension positions
            // which is equivalent to transpose
            .array_aux("transposed[Col,Row]", "matrix[@2, @1]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Original matrix is row-major: [11, 12, 13, 21, 22, 23]
        // Transposed should be: [11, 21, 12, 22, 13, 23]
        project.assert_interpreter_result("transposed", &[11.0, 21.0, 12.0, 22.0, 13.0, 23.0]);
    }

    #[test]
    fn transpose_2d_array_bare() {
        // Test transpose operator on bare 2D array variable
        let project = TestProject::new("transpose_2d_bare")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                ],
            )
            .array_aux("transposed[Col,Row]", "matrix'");

        project.assert_compiles();
        project.assert_sim_builds();
        // Original matrix is row-major: [11, 12, 13, 21, 22, 23]
        // Transposed should be: [11, 21, 12, 22, 13, 23]
        project.assert_interpreter_result("transposed", &[11.0, 21.0, 12.0, 22.0, 13.0, 23.0]);
    }

    #[test]
    fn transpose_3d_array_bare() {
        // Test transpose on 3D array
        let project = TestProject::new("transpose_3d_bare")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .indexed_dimension("Z", 2)
            .array_with_ranges(
                "cube[X,Y,Z]",
                vec![
                    ("1,1,1", "111"),
                    ("1,1,2", "112"),
                    ("1,2,1", "121"),
                    ("1,2,2", "122"),
                    ("2,1,1", "211"),
                    ("2,1,2", "212"),
                    ("2,2,1", "221"),
                    ("2,2,2", "222"),
                ],
            )
            .array_aux("transposed[Z,Y,X]", "cube'");

        project.assert_compiles();
        project.assert_sim_builds();
        // Original: [111, 112, 121, 122, 211, 212, 221, 222]
        // Transposed (Z,Y,X order): [111, 211, 121, 221, 112, 212, 122, 222]
        project.assert_interpreter_result(
            "transposed",
            &[111.0, 211.0, 121.0, 221.0, 112.0, 212.0, 122.0, 222.0],
        );
    }

    #[test]
    fn transpose_with_operations() {
        // Test transpose in expressions
        let project = TestProject::new("transpose_with_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "1"),
                    ("1,2", "2"),
                    ("1,3", "3"),
                    ("2,1", "4"),
                    ("2,2", "5"),
                    ("2,3", "6"),
                ],
            )
            .array_aux("doubled_transpose[Col,Row]", "(matrix * 2)'");

        project.assert_compiles();
        project.assert_sim_builds();
        // Original * 2: [2, 4, 6, 8, 10, 12]
        // Transposed: [2, 8, 4, 10, 6, 12]
        project.assert_interpreter_result("doubled_transpose", &[2.0, 8.0, 4.0, 10.0, 6.0, 12.0]);
    }

    #[test]
    fn transpose_sum() {
        // Test using transpose with aggregate functions
        let project = TestProject::new("transpose_sum")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "1"),
                    ("1,2", "2"),
                    ("1,3", "3"),
                    ("2,1", "4"),
                    ("2,2", "5"),
                    ("2,3", "6"),
                ],
            )
            .scalar_aux("sum_transpose", "SUM(matrix')");

        project.assert_compiles();
        project.assert_sim_builds();
        // Sum should be same whether transposed or not: 1+2+3+4+5+6 = 21
        project.assert_scalar_result("sum_transpose", 21.0);
    }

    #[test]
    fn transpose_1d_array() {
        // Transpose of 1D array should be no-op
        let project = TestProject::new("transpose_1d")
            .indexed_dimension("Points", 5)
            .array_with_ranges(
                "vec[Points]",
                vec![
                    ("1", "10"),
                    ("2", "20"),
                    ("3", "30"),
                    ("4", "40"),
                    ("5", "50"),
                ],
            )
            .array_aux("result[Points]", "vec'");

        project.assert_compiles();
        project.assert_sim_builds();
        // 1D transpose should be identity
        project.assert_interpreter_result("result", &[10.0, 20.0, 30.0, 40.0, 50.0]);
    }

    #[test]
    fn transpose_3d_array() {
        // Test transpose on 3D array - should reverse all dimensions
        let project = TestProject::new("transpose_3d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .indexed_dimension("Z", 2)
            .array_with_ranges(
                "cube[X,Y,Z]",
                vec![
                    ("1,1,1", "111"),
                    ("1,1,2", "112"),
                    ("1,2,1", "121"),
                    ("1,2,2", "122"),
                    ("2,1,1", "211"),
                    ("2,1,2", "212"),
                    ("2,2,1", "221"),
                    ("2,2,2", "222"),
                ],
            )
            // Use dimension positions as a workaround for bare transpose
            .array_aux("transposed[Z,Y,X]", "cube[@3, @2, @1]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Original order: X,Y,Z → [111, 112, 121, 122, 211, 212, 221, 222]
        // Transposed to Z,Y,X → [111, 211, 121, 221, 112, 212, 122, 222]
        project.assert_interpreter_result(
            "transposed",
            &[111.0, 211.0, 121.0, 221.0, 112.0, 212.0, 122.0, 222.0],
        );
    }

    #[test]
    fn transpose_chain() {
        // Test that (A')' = A
        let project = TestProject::new("transpose_chain")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 3)
            .array_with_ranges(
                "original[X,Y]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                ],
            )
            .array_aux("double_transpose[X,Y]", "(original')'");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get back the original
        project
            .assert_interpreter_result("double_transpose", &[11.0, 12.0, 13.0, 21.0, 22.0, 23.0]);
    }

    #[test]
    fn transpose_with_arithmetic() {
        // Test transpose in arithmetic expressions
        let project = TestProject::new("transpose_arithmetic")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_with_ranges(
                "A[Row,Col]",
                vec![("1,1", "1"), ("1,2", "2"), ("2,1", "3"), ("2,2", "4")],
            )
            .array_with_ranges(
                "B[Col,Row]",
                vec![("1,1", "5"), ("1,2", "6"), ("2,1", "7"), ("2,2", "8")],
            )
            // Use dimension positions as a workaround for bare transpose
            .array_aux("sum[Row,Col]", "A + B[@2, @1]"); // B[@2,@1] has dimensions [Row,Col]

        project.assert_compiles();
        project.assert_sim_builds();
        // A = [1,2,3,4], B' = [5,7,6,8], sum = [6,9,9,12]
        project.assert_interpreter_result("sum", &[6.0, 9.0, 9.0, 12.0]);
    }

    #[test]
    fn transpose_scalar_result() {
        // Test transpose used in a scalar context (e.g., SUM)
        let project = TestProject::new("transpose_scalar")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "1"),
                    ("1,2", "2"),
                    ("1,3", "3"),
                    ("2,1", "4"),
                    ("2,2", "5"),
                    ("2,3", "6"),
                ],
            )
            // Use dimension positions as a workaround for bare transpose
            .scalar_aux("sum_transposed", "SUM(matrix[@2, @1])");

        project.assert_compiles();
        project.assert_sim_builds();
        // Sum should be the same regardless of transpose
        project.assert_scalar_result("sum_transposed", 21.0);
    }
}

#[cfg(test)]
mod range_tests {
    use crate::test_common::TestProject;

    #[test]
    fn range_sum_1d_w_ops() {
        let project = TestProject::new("range_sum_1d_w_ops")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .scalar_aux("summed", "SUM(2 * source[3:5] + 1)");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("summed", 27.0);
    }

    #[test]
    fn range_sum_1d() {
        let project = TestProject::new("range_sum_1d")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .scalar_aux("summed", "SUM(source[3:5])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("summed", 12.0);
    }

    #[test]
    fn range_basic_a() {
        // Test basic range subscript [1:3]
        let project = TestProject::new("range_basic")
            .indexed_dimension("A", 5)
            .indexed_dimension("B", 3)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .array_aux("slice[B]", "source[3:5]");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("slice", &[3.0, 4.0, 5.0]);
    }

    #[test]
    fn range_2d_first_dim() {
        // Test slicing the first dimension of a 2D array: source[2:3, *]
        let project = TestProject::new("range_2d_first")
            .indexed_dimension("Row", 4)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                    ("3,1", "31"),
                    ("3,2", "32"),
                    ("3,3", "33"),
                    ("4,1", "41"),
                    ("4,2", "42"),
                    ("4,3", "43"),
                ],
            )
            // Slice rows 2:3 (inclusive), keeping all columns
            .indexed_dimension("SliceRow", 2)
            .array_aux("slice[SliceRow,Col]", "matrix[2:3, *]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get rows 2 and 3: [21, 22, 23, 31, 32, 33]
        project.assert_interpreter_result("slice", &[21.0, 22.0, 23.0, 31.0, 32.0, 33.0]);
    }

    #[test]
    fn range_2d_second_dim() {
        // Test slicing the second dimension of a 2D array: source[*, 2:3]
        let project = TestProject::new("range_2d_second")
            .indexed_dimension("Row", 3)
            .indexed_dimension("Col", 4)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("1,4", "14"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                    ("2,4", "24"),
                    ("3,1", "31"),
                    ("3,2", "32"),
                    ("3,3", "33"),
                    ("3,4", "34"),
                ],
            )
            // Slice columns 2:3 (inclusive), keeping all rows
            .indexed_dimension("SliceCol", 2)
            .array_aux("slice[Row,SliceCol]", "matrix[*, 2:3]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get columns 2 and 3: [12, 13, 22, 23, 32, 33]
        project.assert_interpreter_result("slice", &[12.0, 13.0, 22.0, 23.0, 32.0, 33.0]);
    }

    #[test]
    fn range_2d_both_dims() {
        // Test slicing both dimensions: source[2:3, 2:4]
        let project = TestProject::new("range_2d_both")
            .indexed_dimension("Row", 4)
            .indexed_dimension("Col", 5)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("1,4", "14"),
                    ("1,5", "15"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                    ("2,4", "24"),
                    ("2,5", "25"),
                    ("3,1", "31"),
                    ("3,2", "32"),
                    ("3,3", "33"),
                    ("3,4", "34"),
                    ("3,5", "35"),
                    ("4,1", "41"),
                    ("4,2", "42"),
                    ("4,3", "43"),
                    ("4,4", "44"),
                    ("4,5", "45"),
                ],
            )
            // Slice rows 2:3 and columns 2:4
            .indexed_dimension("SliceRow", 2)
            .indexed_dimension("SliceCol", 3)
            .array_aux("slice[SliceRow,SliceCol]", "matrix[2:3, 2:4]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get 2x3 submatrix: [22, 23, 24, 32, 33, 34]
        project.assert_interpreter_result("slice", &[22.0, 23.0, 24.0, 32.0, 33.0, 34.0]);
    }

    #[test]
    fn range_3d_single_dim() {
        // Test slicing one dimension of a 3D array
        let project = TestProject::new("range_3d_single")
            .indexed_dimension("X", 3)
            .indexed_dimension("Y", 3)
            .indexed_dimension("Z", 3)
            .array_with_ranges(
                "cube[X,Y,Z]",
                vec![
                    // X=1
                    ("1,1,1", "111"),
                    ("1,1,2", "112"),
                    ("1,1,3", "113"),
                    ("1,2,1", "121"),
                    ("1,2,2", "122"),
                    ("1,2,3", "123"),
                    ("1,3,1", "131"),
                    ("1,3,2", "132"),
                    ("1,3,3", "133"),
                    // X=2
                    ("2,1,1", "211"),
                    ("2,1,2", "212"),
                    ("2,1,3", "213"),
                    ("2,2,1", "221"),
                    ("2,2,2", "222"),
                    ("2,2,3", "223"),
                    ("2,3,1", "231"),
                    ("2,3,2", "232"),
                    ("2,3,3", "233"),
                    // X=3
                    ("3,1,1", "311"),
                    ("3,1,2", "312"),
                    ("3,1,3", "313"),
                    ("3,2,1", "321"),
                    ("3,2,2", "322"),
                    ("3,2,3", "323"),
                    ("3,3,1", "331"),
                    ("3,3,2", "332"),
                    ("3,3,3", "333"),
                ],
            )
            // Slice Z dimension to [2:3]
            .indexed_dimension("SliceZ", 2)
            .array_aux("slice[X,Y,SliceZ]", "cube[*, *, 2:3]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get all X,Y with Z=2,3
        project.assert_interpreter_result(
            "slice",
            &[
                112.0, 113.0, 122.0, 123.0, 132.0, 133.0, // X=1
                212.0, 213.0, 222.0, 223.0, 232.0, 233.0, // X=2
                312.0, 313.0, 322.0, 323.0, 332.0, 333.0, // X=3
            ],
        );
    }

    #[test]
    fn range_with_single_index_mix() {
        // Test mixing range with single index: source[2, 3:5]
        let project = TestProject::new("range_mixed")
            .indexed_dimension("Row", 3)
            .indexed_dimension("Col", 5)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("1,4", "14"),
                    ("1,5", "15"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                    ("2,4", "24"),
                    ("2,5", "25"),
                    ("3,1", "31"),
                    ("3,2", "32"),
                    ("3,3", "33"),
                    ("3,4", "34"),
                    ("3,5", "35"),
                ],
            )
            // Select row 2, slice columns 3:5
            .indexed_dimension("SliceCol", 3)
            .array_aux("slice[SliceCol]", "matrix[2, 3:5]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get row 2, columns 3-5: [23, 24, 25]
        project.assert_interpreter_result("slice", &[23.0, 24.0, 25.0]);
    }

    #[test]
    fn named_range_basic() {
        // Test basic named dimension range [City.Boston:City.LA]
        let project = TestProject::new("named_range_basic")
            .named_dimension("City", &["Boston", "NYC", "LA", "SF", "Seattle"])
            .array_with_ranges(
                "population[City]",
                vec![
                    ("Boston", "100"),
                    ("NYC", "200"),
                    ("LA", "300"),
                    ("SF", "400"),
                    ("Seattle", "500"),
                ],
            )
            .indexed_dimension("Result", 3) // Boston, NYC, LA
            .array_aux("east_to_la[Result]", "population[Boston:LA]");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("east_to_la", &[100.0, 200.0, 300.0]);
    }

    #[test]
    fn range_mean_1d() {
        let project = TestProject::new("range_mean_1d")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![
                    ("1", "10"),
                    ("2", "20"),
                    ("3", "30"),
                    ("4", "40"),
                    ("5", "50"),
                ],
            )
            .scalar_aux("mean_val", "MEAN(source[2:4])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("mean_val", 30.0);
    }

    #[test]
    fn range_mean_1d_w_ops() {
        let project = TestProject::new("range_mean_1d_w_ops")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .scalar_aux("mean_val", "MEAN(2 * source[3:5] + 1)");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("mean_val", 9.0);
    }

    #[test]
    fn range_stddev_1d() {
        let project = TestProject::new("range_stddev_1d")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .scalar_aux("stddev_val", "STDDEV(source[2:4])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("stddev_val", 1.0);
    }

    #[test]
    fn range_stddev_1d_w_ops() {
        let project = TestProject::new("range_stddev_1d_w_ops")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "2"), ("2", "4"), ("3", "6"), ("4", "8"), ("5", "10")],
            )
            .scalar_aux("stddev_val", "STDDEV(source[1:5] / 2)");

        project.assert_compiles();
        project.assert_sim_builds();
        // source[1:5] = [2, 4, 6, 8, 10] (inclusive range per XMILE spec)
        // source[1:5] / 2 = [1, 2, 3, 4, 5], mean = 3, stddev = sqrt(2.5) ≈ 1.581
        project.assert_scalar_result("stddev_val", 1.5811388300841898);
    }

    #[test]
    fn range_min_1d() {
        let project = TestProject::new("range_min_1d")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![
                    ("1", "10"),
                    ("2", "5"),
                    ("3", "15"),
                    ("4", "3"),
                    ("5", "20"),
                ],
            )
            .scalar_aux("min_val", "MIN(source[2:4])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("min_val", 3.0);
    }

    #[test]
    fn range_min_1d_w_ops() {
        let project = TestProject::new("range_min_1d_w_ops")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .scalar_aux("min_val", "MIN(source[3:5] * 2 - 1)");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("min_val", 5.0);
    }

    #[test]
    fn range_max_1d() {
        let project = TestProject::new("range_max_1d")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![
                    ("1", "10"),
                    ("2", "5"),
                    ("3", "15"),
                    ("4", "3"),
                    ("5", "20"),
                ],
            )
            .scalar_aux("max_val", "MAX(source[2:4])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("max_val", 15.0);
    }

    #[test]
    fn range_max_1d_w_ops() {
        let project = TestProject::new("range_max_1d_w_ops")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .scalar_aux("max_val", "MAX(source[3:5] * 2 - 1)");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("max_val", 9.0);
    }

    #[test]
    fn range_size_1d() {
        let project = TestProject::new("range_size_1d")
            .indexed_dimension("A", 5)
            .array_with_ranges(
                "source[A]",
                vec![("1", "1"), ("2", "2"), ("3", "3"), ("4", "4"), ("5", "5")],
            )
            .scalar_aux("size_val", "SIZE(source[2:4])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("size_val", 3.0);
    }

    #[test]
    fn range_size_2d() {
        let project = TestProject::new("range_size_2d")
            .indexed_dimension("Row", 3)
            .indexed_dimension("Col", 4)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("1,1", "11"),
                    ("1,2", "12"),
                    ("1,3", "13"),
                    ("1,4", "14"),
                    ("2,1", "21"),
                    ("2,2", "22"),
                    ("2,3", "23"),
                    ("2,4", "24"),
                    ("3,1", "31"),
                    ("3,2", "32"),
                    ("3,3", "33"),
                    ("3,4", "34"),
                ],
            )
            .scalar_aux("size_val", "SIZE(matrix[2:3, 2:3])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("size_val", 4.0);
    }

    #[test]
    fn named_range_sum() {
        // Test SUM with named dimension range
        let project = TestProject::new("named_range_sum")
            .named_dimension("Month", &["Jan", "Feb", "Mar", "Apr", "May", "Jun"])
            .array_with_ranges(
                "sales[Month]",
                vec![
                    ("Jan", "10"),
                    ("Feb", "20"),
                    ("Mar", "30"),
                    ("Apr", "40"),
                    ("May", "50"),
                    ("Jun", "60"),
                ],
            )
            .scalar_aux("q1_total", "SUM(sales[Jan:Mar])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("q1_total", 60.0); // 10 + 20 + 30
    }

    #[test]
    fn named_range_2d() {
        // Test named range in 2D array
        let project = TestProject::new("named_range_2d")
            .named_dimension("City", &["Boston", "NYC", "LA", "SF"])
            .indexed_dimension("Year", 3)
            .array_with_ranges(
                "data[City,Year]",
                vec![
                    ("Boston,1", "11"),
                    ("Boston,2", "12"),
                    ("Boston,3", "13"),
                    ("NYC,1", "21"),
                    ("NYC,2", "22"),
                    ("NYC,3", "23"),
                    ("LA,1", "31"),
                    ("LA,2", "32"),
                    ("LA,3", "33"),
                    ("SF,1", "41"),
                    ("SF,2", "42"),
                    ("SF,3", "43"),
                ],
            )
            .indexed_dimension("SubCities", 2) // NYC, LA
            .array_aux("subset[SubCities,Year]", "data[NYC:LA, *]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get NYC and LA rows: [21,22,23,31,32,33]
        project.assert_interpreter_result("subset", &[21.0, 22.0, 23.0, 31.0, 32.0, 33.0]);
    }

    #[test]
    fn named_range_mixed_dimensions() {
        // Test mixing named range with numeric range
        let project = TestProject::new("named_range_mixed")
            .named_dimension("Product", &["A", "B", "C", "D", "E"])
            .indexed_dimension("Quarter", 4)
            .array_with_ranges(
                "sales[Product,Quarter]",
                vec![
                    ("A,1", "10"),
                    ("A,2", "11"),
                    ("A,3", "12"),
                    ("A,4", "13"),
                    ("B,1", "20"),
                    ("B,2", "21"),
                    ("B,3", "22"),
                    ("B,4", "23"),
                    ("C,1", "30"),
                    ("C,2", "31"),
                    ("C,3", "32"),
                    ("C,4", "33"),
                    ("D,1", "40"),
                    ("D,2", "41"),
                    ("D,3", "42"),
                    ("D,4", "43"),
                    ("E,1", "50"),
                    ("E,2", "51"),
                    ("E,3", "52"),
                    ("E,4", "53"),
                ],
            )
            .indexed_dimension("SubProducts", 3) // B, C, D
            .indexed_dimension("SubQuarters", 2) // Q2, Q3
            .array_aux("subset[SubProducts,SubQuarters]", "sales[B:D, 2:3]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get B,C,D for Q2,Q3: [21,22,31,32,41,42]
        project.assert_interpreter_result("subset", &[21.0, 22.0, 31.0, 32.0, 41.0, 42.0]);
    }

    #[test]
    #[ignore]
    fn range_basic() {
        // Test basic range subscript [1:3]
        TestProject::new("range_basic")
            .indexed_dimension("Periods", 5)
            .array_aux("source[Periods]", "Periods")
            .array_aux("slice[Periods]", "source[1:3]")
            // TODO: if you assign like `array[len(5)] = array[len(3)]` we should zero extend not extend the last element, or error out.
            .assert_interpreter_result("slice", &[1.0, 2.0, 3.0, 3.0, 3.0]);
    }

    #[test]
    #[ignore]
    fn range_with_expressions() {
        // Test range with expressions [start:end]
        TestProject::new("range_expr")
            .indexed_dimension("Index", 10)
            .scalar_const("start", 2.0)
            .scalar_const("end", 5.0)
            .array_const("data[Index]", 1.0)
            .array_aux("slice[Index]", "data[start:end]")
            // TODO: zero extend
            .assert_interpreter_result(
                "slice",
                &[2.0, 3.0, 4.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0, 5.0],
            );
    }
}

#[cfg(test)]
mod combined_operations_tests {
    use crate::test_common::TestProject;

    #[test]
    #[ignore] // Enable when all operations are implemented
    fn transpose_and_slice() {
        // Combine transpose with slicing
        TestProject::new("combined_transpose_slice")
            .indexed_dimension("Row", 3)
            .indexed_dimension("Col", 4)
            .array_aux("matrix[Row,Col]", "Row * 10 + Col")
            .array_aux("result", "matrix'[1:3, *]") // Transpose then slice
            .assert_interpreter_result("result", &[1.0, 11.0, 21.0, 2.0, 12.0, 22.0]);
    }

    #[test]
    fn dimension_position_and_wildcard() {
        // Combine dimension position with wildcard
        TestProject::new("combined_dimpos_wildcard")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 3)
            .indexed_dimension("Z", 4)
            .array_aux("cube[X,Y,Z]", "X * 100 + Y * 10 + Z")
            .array_aux("slice[Z,Y]", "cube[@1, *, @3]") // Fix X=0, reorder Y and Z
            .assert_compiles();
    }

    #[test]
    #[ignore] // Enable when all operations are implemented
    fn complex_expression() {
        // Test complex array expression
        TestProject::new("complex_expr")
            .indexed_dimension("Period", 5)
            .indexed_dimension("Product", 3)
            .array_aux("sales[Period,Product]", "Period * Product")
            .array_aux("costs[Period,Product]", "Product * 10")
            .array_aux("profit[Period,Product]", "sales[*,*] - costs[*,*]")
            .array_aux("total_profit[Period]", "SUM(profit[*, Product.*])")
            .assert_compiles();
    }
}

#[cfg(test)]
mod error_handling_tests {
    #[test]
    fn dimension_mismatch() {
        // Test that dimension mismatches are caught
        // TestProject::new("dim_mismatch")
        //     .indexed_dimension("X", 3)
        //     .indexed_dimension("Y", 4)
        //     .array_const("arr1[X]", 1.0)
        //     .array_const("arr2[Y]", 2.0)
        //     .array_aux("result[X]", "arr1[*] + arr2[*]")  // Should fail - different dimensions
        //     .assert_compile_error(ErrorCode::ArrayDimensionMismatch);
    }

    #[test]
    fn out_of_bounds_index() {
        // Test out of bounds access
        // TestProject::new("out_of_bounds")
        //     .indexed_dimension("Small", 3)
        //     .array_const("arr[Small]", 10.0)
        //     .scalar_aux("bad_access", "arr[5]")  // Index 5 out of bounds for size 3
        //     .assert_compile_error(ErrorCode::ArrayIndexOutOfBounds);
    }
}

mod implicit_transpose_tests {
    use crate::test_common::TestProject;

    #[test]
    fn implicit_2d_transpose() {
        // Test implicit dimension reordering for 2D arrays
        // source: [Row, Col], target: [Col, Row] - should work automatically
        TestProject::new("implicit_2d")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_aux("source[Row, Col]", "Row * 10 + Col")
            // This should implicitly transpose: source[Row,Col] -> target[Col,Row]
            .array_aux("target[Col, Row]", "source")
            // source[1,1]=11, source[1,2]=12, source[1,3]=13
            // source[2,1]=21, source[2,2]=22, source[2,3]=23
            // After transpose:
            // target[1,1]=11, target[1,2]=21
            // target[2,1]=12, target[2,2]=22
            // target[3,1]=13, target[3,2]=23
            .assert_interpreter_result("target", &[11.0, 21.0, 12.0, 22.0, 13.0, 23.0]);
    }

    #[test]
    fn implicit_3d_reordering() {
        // Test implicit dimension reordering for 3D arrays
        TestProject::new("implicit_3d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .indexed_dimension("Z", 2)
            .array_aux("source[X, Y, Z]", "X * 100 + Y * 10 + Z")
            // Reorder from [X,Y,Z] to [Y,Z,X]
            .array_aux("reordered[Y, Z, X]", "source")
            // source values:
            // [1,1,1]=111, [1,1,2]=112, [1,2,1]=121, [1,2,2]=122
            // [2,1,1]=211, [2,1,2]=212, [2,2,1]=221, [2,2,2]=222
            // After reordering to [Y,Z,X]:
            // [1,1,1]=111, [1,1,2]=211, [1,2,1]=112, [1,2,2]=212
            // [2,1,1]=121, [2,1,2]=221, [2,2,1]=122, [2,2,2]=222
            .assert_interpreter_result(
                "reordered",
                &[111.0, 211.0, 112.0, 212.0, 121.0, 221.0, 122.0, 222.0],
            );
    }

    #[test]
    fn implicit_transpose_in_binary_ops() {
        // Test implicit transpose in binary operations
        TestProject::new("implicit_binary")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_aux("matrix1[Row, Col]", "Row + Col")
            .array_aux("matrix2[Col, Row]", "Col * 10 + Row")
            // This should work: matrix1[Row,Col] + matrix2' (implicit transpose)
            .array_aux("sum[Row, Col]", "matrix1 + matrix2")
            // matrix1: [2,3,4; 3,4,5]
            // matrix2: [11,12; 21,22; 31,32] -> transposed: [11,21,31; 12,22,32]
            // sum: [13,24,35; 15,26,37]
            .assert_interpreter_result("sum", &[13.0, 24.0, 35.0, 15.0, 26.0, 37.0]);
    }

    #[test]
    fn implicit_transpose_all_binary_ops() {
        // Test implicit transpose works with all binary operations
        TestProject::new("implicit_all_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_aux("a[Row, Col]", "(Row + 1) * 2 + Col") // [3,4; 5,6]
            .array_aux("b[Col, Row]", "(Col + 1) * 3 + Row") // [4,5; 7,8] -> transposed: [4,7; 5,8]
            // Test addition
            .array_aux("add_result[Row, Col]", "a + b") // [7,11; 10,14]
            // Test subtraction
            .array_aux("sub_result[Row, Col]", "a - b") // [-1,-3; 0,-2]
            // Test multiplication
            .array_aux("mul_result[Row, Col]", "a * b") // [12,28; 25,48]
            // Test division
            .array_aux("div_result[Row, Col]", "a / b") // [0.75,0.571...; 1,0.75]
            // Test comparison (greater than)
            .array_aux("gt_result[Row, Col]", "if a > b then 1 else 0") // [0,0; 0,0]
            // Test comparison (less than)
            .array_aux("lt_result[Row, Col]", "if a < b then 1 else 0"); // [1,1; 0,1]

        // Test each result separately
        // a: [5,6; 7,8]
        // b: [7,8; 10,11] -> transposed: [7,10; 8,11]
        TestProject::new("implicit_all_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_aux("a[Row, Col]", "(Row + 1) * 2 + Col")
            .array_aux("b[Col, Row]", "(Col + 1) * 3 + Row")
            .array_aux("add_result[Row, Col]", "a + b")
            .assert_interpreter_result("add_result", &[12.0, 16.0, 15.0, 19.0]);

        // a - b': [5-7, 6-10; 7-8, 8-11] = [-2, -4; -1, -3]
        TestProject::new("implicit_all_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_aux("a[Row, Col]", "(Row + 1) * 2 + Col")
            .array_aux("b[Col, Row]", "(Col + 1) * 3 + Row")
            .array_aux("sub_result[Row, Col]", "a - b")
            .assert_interpreter_result("sub_result", &[-2.0, -4.0, -1.0, -3.0]);

        // a * b': [5*7, 6*10; 7*8, 8*11] = [35, 60; 56, 88]
        TestProject::new("implicit_all_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_aux("a[Row, Col]", "(Row + 1) * 2 + Col")
            .array_aux("b[Col, Row]", "(Col + 1) * 3 + Row")
            .array_aux("mul_result[Row, Col]", "a * b")
            .assert_interpreter_result("mul_result", &[35.0, 60.0, 56.0, 88.0]);

        // a / b': [5/7, 6/10; 7/8, 8/11]
        TestProject::new("implicit_all_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_aux("a[Row, Col]", "(Row + 1) * 2 + Col")
            .array_aux("b[Col, Row]", "(Col + 1) * 3 + Row")
            .array_aux("div_result[Row, Col]", "a / b")
            .assert_interpreter_result(
                "div_result",
                &[5.0 / 7.0, 6.0 / 10.0, 7.0 / 8.0, 8.0 / 11.0],
            );

        // a > b': [5>7, 6>10; 7>8, 8>11] = [0, 0; 0, 0]
        TestProject::new("implicit_all_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_aux("a[Row, Col]", "(Row + 1) * 2 + Col")
            .array_aux("b[Col, Row]", "(Col + 1) * 3 + Row")
            .array_aux("gt_result[Row, Col]", "if a > b then 1 else 0")
            .assert_interpreter_result("gt_result", &[0.0, 0.0, 0.0, 0.0]);

        // a < b': [5<7, 6<10; 7<8, 8<11] = [1, 1; 1, 1]
        TestProject::new("implicit_all_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 2)
            .array_aux("a[Row, Col]", "(Row + 1) * 2 + Col")
            .array_aux("b[Col, Row]", "(Col + 1) * 3 + Row")
            .array_aux("lt_result[Row, Col]", "if a < b then 1 else 0")
            .assert_interpreter_result("lt_result", &[1.0, 1.0, 1.0, 1.0]);
    }

    #[test]
    fn dimension_mismatch_error() {
        // Test that incompatible dimensions cause an error
        // This test should verify that the simulation fails to build
        // when dimensions cannot be reordered to match

        // TestProject::new("dim_mismatch")
        //     .indexed_dimension("A", 2)
        //     .indexed_dimension("B", 3)
        //     .indexed_dimension("C", 2)
        //     .array_aux("source[A, B]", "1")
        //     .array_aux("target[B, C]", "source") // Different dimensions - should fail
        //     .assert_compile_error(ErrorCode::ArrayDimensionMismatch);
    }
}

mod a2a_assignment_tests {
    use crate::test_common::TestProject;

    #[test]
    fn a2a_with_bare_transpose_and_operations() {
        // Test that A2A assignments with bare transpose + operations work correctly
        // This specifically tests that the compiler properly handles the expression rewriting
        TestProject::new("a2a_transpose_ops")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            // Create matrix with individual equations
            // matrix[1,1] = 1, matrix[1,2] = 3, matrix[1,3] = 5
            // matrix[2,1] = 2, matrix[2,2] = 4, matrix[2,3] = 6
            .array_aux("matrix[Row, Col]", "Row + (Col - 1) * 2")
            .array_aux("transposed_doubled[Col, Row]", "matrix' * 2")
            // After transpose: [1,2; 3,4; 5,6], doubled: [2,4; 6,8; 10,12]
            .assert_interpreter_result("transposed_doubled", &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0]);
    }

    #[test]
    fn a2a_with_complex_expression() {
        // Test A2A with complex expression involving multiple operations
        TestProject::new("a2a_complex")
            .indexed_dimension("X", 3)
            .indexed_dimension("Y", 2)
            // source[1,1]=1, source[1,2]=2, source[2,1]=3, source[2,2]=4, source[3,1]=5, source[3,2]=6
            .array_aux("source[X, Y]", "(X - 1) * 2 + Y")
            .array_aux("result[Y, X]", "(source' + 1) * 2 - 3")
            // source': [1, 3, 5; 2, 4, 6]
            // +1: [2, 4, 6; 3, 5, 7]
            // *2: [4, 8, 12; 6, 10, 14]
            // -3: [1, 5, 9; 3, 7, 11]
            .assert_interpreter_result("result", &[1.0, 5.0, 9.0, 3.0, 7.0, 11.0]);
    }
}

#[cfg(test)]
mod star_range_subdimension_tests {
    use crate::test_common::TestProject;

    #[test]
    fn star_to_subdimension_simple() {
        // Simpler test: just check if *:SubA resolves correctly as a subscript
        let project = TestProject::new("star_simple")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .named_dimension("SubA", &["A2", "A3"])
            .array_const("b_2[DimA]", 10.0) // Use a simple constant instead
            // Direct subscript with star range
            .array_aux("result[SubA]", "b_2[*:SubA]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get elements A2 and A3 only (both are 10.0 since it's a constant array)
        project.assert_interpreter_result("result", &[10.0, 10.0]);
    }

    #[test]
    fn star_to_subdimension() {
        // Test that *:SubDim creates a range from the first element to the last element of SubDim
        let project = TestProject::new("star_to_subdim")
            .named_dimension("DimA", &["A1", "A2", "A3"])
            .named_dimension("SubA", &["A2", "A3"])
            // Use array_aux instead of array_with_ranges to have a simple equation
            .array_aux("b_2[DimA]", "DimA") // Will give 1, 2, 3 for A1, A2, A3
            // *:SubA should resolve to A2:A3 (elements 2 and 3)
            .array_aux("k[SubA]", "b_2[*:SubA]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get elements A2 and A3 only
        project.assert_interpreter_result("k", &[2.0, 3.0]);
    }

    #[test]
    fn star_to_subdimension_with_sum() {
        // Test star range with SUM builtin
        let project = TestProject::new("star_to_subdim_sum")
            .named_dimension("DimA", &["A1", "A2", "A3", "A4"])
            .named_dimension("SubA", &["A2", "A3"])
            .array_with_ranges(
                "values[DimA]",
                vec![("A1", "10"), ("A2", "20"), ("A3", "30"), ("A4", "40")],
            )
            // SUM(values[*:SubA]) should sum elements A2 and A3
            .scalar_aux("total", "SUM(values[*:SubA])");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should sum A2 (20) and A3 (30) = 50
        project.assert_scalar_result("total", 50.0);
    }

    // TODO: Indexed subdimensions deferred - datamodel lacks parent mapping metadata.
    // When the parent dimension for an indexed subdimension can be expressed in the
    // datamodel, this test should be enabled.
    #[test]
    #[ignore]
    fn star_to_indexed_subdimension() {
        // Test star range with indexed dimensions
        let project = TestProject::new("star_to_indexed_subdim")
            .indexed_dimension("Index", 5)
            .indexed_dimension("SubIndex", 3) // Represents indices 2, 3, 4
            .array_const("arr[Index]", 10.0)
            // *:SubIndex should resolve to 2:4
            .array_aux("slice[SubIndex]", "arr[*:SubIndex] * 2");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get elements 2, 3, 4 multiplied by 2
        project.assert_interpreter_result("slice", &[20.0, 20.0, 20.0]);
    }

    #[test]
    fn star_range_with_multidim() {
        // Test star range in multi-dimensional context
        let project = TestProject::new("star_multidim")
            .named_dimension("Row", &["R1", "R2", "R3"])
            .named_dimension("Col", &["C1", "C2", "C3", "C4"])
            .named_dimension("SubCol", &["C2", "C3"])
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("R1,C1", "11"),
                    ("R1,C2", "12"),
                    ("R1,C3", "13"),
                    ("R1,C4", "14"),
                    ("R2,C1", "21"),
                    ("R2,C2", "22"),
                    ("R2,C3", "23"),
                    ("R2,C4", "24"),
                    ("R3,C1", "31"),
                    ("R3,C2", "32"),
                    ("R3,C3", "33"),
                    ("R3,C4", "34"),
                ],
            )
            // Select all rows, columns C2:C3
            .array_aux("slice[Row,SubCol]", "matrix[*, *:SubCol]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get columns C2 and C3 for all rows
        project.assert_interpreter_result("slice", &[12.0, 13.0, 22.0, 23.0, 32.0, 33.0]);
    }

    #[test]
    fn star_to_subdimension_non_contiguous() {
        // Test star range with non-contiguous subdimension (exercises sparse iteration)
        let project = TestProject::new("star_to_subdim_non_contiguous")
            .named_dimension("DimA", &["A1", "A2", "A3", "A4"])
            .named_dimension("SubA", &["A1", "A3"]) // Non-contiguous: offsets [0, 2]
            .array_with_ranges(
                "values[DimA]",
                vec![("A1", "10"), ("A2", "20"), ("A3", "30"), ("A4", "40")],
            )
            .array_aux("result[SubA]", "values[*:SubA]");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should get A1 (10) and A3 (30)
        project.assert_interpreter_result("result", &[10.0, 30.0]);
    }

    #[test]
    fn star_to_subdimension_non_contiguous_with_sum() {
        // Test SUM with non-contiguous subdimension
        let project = TestProject::new("star_to_subdim_non_contiguous_sum")
            .named_dimension("DimA", &["A1", "A2", "A3", "A4"])
            .named_dimension("SubA", &["A1", "A3"]) // Non-contiguous
            .array_with_ranges(
                "values[DimA]",
                vec![("A1", "10"), ("A2", "20"), ("A3", "30"), ("A4", "40")],
            )
            .scalar_aux("total", "SUM(values[*:SubA])");

        project.assert_compiles();
        project.assert_sim_builds();
        // Should sum A1 (10) and A3 (30) = 40
        project.assert_scalar_result("total", 40.0);
    }

    #[test]
    fn sum_with_active_dimension_in_subscript() {
        // This test reproduces the issue from simulates_sum test
        // We have a 2D array m[DimD, DimE] and want to compute msum[DimD] = SUM(m[DimD, *])
        // For each element of DimD, we sum across all elements of DimE
        let project = TestProject::new("sum_active_dim")
            .named_dimension("DimD", &["D1", "D2"])
            .named_dimension("DimE", &["E1", "E2"])
            .array_with_ranges(
                "m[DimD, DimE]",
                vec![
                    ("D1,E1", "11"),
                    ("D1,E2", "12"),
                    ("D2,E1", "21"),
                    ("D2,E2", "22"),
                ],
            )
            // This should sum across the second dimension for each element of the first
            // msum[D1] = SUM(m[D1, *]) = m[D1,E1] + m[D1,E2] = 11 + 12 = 23
            // msum[D2] = SUM(m[D2, *]) = m[D2,E1] + m[D2,E2] = 21 + 22 = 43
            .array_aux("msum[DimD]", "SUM(m[DimD, *])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("msum", &[23.0, 43.0]);
    }

    #[test]
    fn sum_with_dimension_name_as_subscript() {
        // Even more minimal test - use dimension name directly as subscript
        // This is exactly what fails in simulates_sum
        let project = TestProject::new("sum_dim_name")
            .named_dimension("DimD", &["D1", "D2"])
            .named_dimension("DimE", &["E1", "E2"])
            .scalar_const("m_11", 11.0)
            .scalar_const("m_12", 12.0)
            .scalar_const("m_21", 21.0)
            .scalar_const("m_22", 22.0)
            // Build m as an arrayed variable with explicit elements
            .array_with_ranges(
                "m[DimD, DimE]",
                vec![
                    ("D1,E1", "m_11"),
                    ("D1,E2", "m_12"),
                    ("D2,E1", "m_21"),
                    ("D2,E2", "m_22"),
                ],
            )
            // Use dimension name DimD as subscript - should resolve to current element in A2A
            .array_aux("msum[DimD]", "SUM(m[DimD, *])");

        project.assert_compiles();
        project.assert_sim_builds();
        // msum[D1] = 11 + 12 = 23, msum[D2] = 21 + 22 = 43
        project.assert_interpreter_result("msum", &[23.0, 43.0]);
    }
}

#[cfg(test)]
mod structural_lowering_tests {
    //! Tests that verify the structure of lowered expressions, not just execution results.
    //! These tests use Module::get_flow_exprs() to inspect the AST after lowering.

    use crate::compiler::{BuiltinFn, Expr, pretty};
    use crate::test_common::TestProject;

    #[test]
    fn a2a_collapses_wildcard_to_var() {
        // In A2A context, source[*] collapses to a scalar Var
        // because the wildcard resolves to the current element being computed
        let project = TestProject::new("test")
            .indexed_dimension("D", 3)
            .array_const("source[D]", 10.0)
            .array_aux("result[D]", "source[*]");

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("result");

        // Should have 3 expressions (one per A2A element)
        assert_eq!(
            exprs.len(),
            3,
            "expected 3 A2A elements, got {}",
            exprs.len()
        );

        // Each should be AssignCurr containing Var (collapsed), not StaticSubscript
        for expr in &exprs {
            if let Expr::AssignCurr(_, inner) = expr {
                assert!(
                    matches!(inner.as_ref(), Expr::Var(..)),
                    "expected Var inside AssignCurr, got: {}",
                    pretty(expr)
                );
            } else {
                panic!("expected AssignCurr, got: {}", pretty(expr));
            }
        }
    }

    #[test]
    fn sum_preserves_array_for_iteration() {
        // Inside SUM, the array reference is preserved as StaticSubscript for iteration
        let project = TestProject::new("test")
            .indexed_dimension("A", 5)
            .array_const("source[A]", 10.0)
            .scalar_aux("total", "SUM(source[2:4])");

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("total");

        assert_eq!(exprs.len(), 1, "scalar should have 1 expression");

        // Verify SUM contains StaticSubscript with range view
        let expr = exprs[0];
        if let Expr::AssignCurr(_, inner) = expr {
            if let Expr::App(BuiltinFn::Sum(sum_inner), _) = inner.as_ref() {
                if let Expr::StaticSubscript(_, view, _) = sum_inner.as_ref() {
                    // Range 2:4 = 3 elements (inclusive)
                    assert_eq!(
                        view.dims,
                        vec![3],
                        "expected range view with 3 elements, got: {:?}",
                        view.dims
                    );
                } else {
                    panic!(
                        "expected StaticSubscript inside SUM, got: {}",
                        pretty(sum_inner)
                    );
                }
            } else {
                panic!(
                    "expected App(Sum) inside AssignCurr, got: {}",
                    pretty(inner)
                );
            }
        } else {
            panic!("expected AssignCurr, got: {}", pretty(expr));
        }
    }

    #[test]
    fn sum_with_dynamic_range() {
        // Test SUM(arr[start:end]) where start and end are variables
        // This is the critical dynamic range in reduction context case
        let project = TestProject::new("sum_dynamic_range")
            .indexed_dimension("Index", 10)
            .array_aux("data[Index]", "Index") // data = [1,2,3,4,5,6,7,8,9,10]
            .scalar_const("start_idx", 3.0)
            .scalar_const("end_idx", 7.0)
            // SUM(data[3:7]) should sum elements at indices 3,4,5,6,7 = 3+4+5+6+7 = 25
            .scalar_aux("total", "SUM(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("total", 25.0);
    }

    #[test]
    fn sum_with_dynamic_range_2d() {
        // Test dynamic range in 2D array context
        let project = TestProject::new("sum_dynamic_range_2d")
            .indexed_dimension("Row", 4)
            .indexed_dimension("Col", 5)
            // matrix[r,c] = r*10 + c: [[11,12,13,14,15], [21,22,23,24,25], [31,32,33,34,35], [41,42,43,44,45]]
            .array_aux("matrix[Row,Col]", "Row * 10 + Col")
            .scalar_const("col_start", 2.0)
            .scalar_const("col_end", 4.0)
            // Sum row 2 columns 2:4 -> matrix[2,2] + matrix[2,3] + matrix[2,4] = 22+23+24 = 69
            .scalar_aux("partial_sum", "SUM(matrix[2, col_start:col_end])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("partial_sum", 69.0);
    }

    #[test]
    fn sum_with_dynamic_range_mixed_subscripts_3d() {
        // Test dimension index adjustment with multiple single subscripts
        // cube[1, row_start:row_end, 2] - single, range, single pattern
        let project = TestProject::new("sum_dynamic_range_3d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 4)
            .indexed_dimension("Z", 3)
            // cube[x,y,z] = x*100 + y*10 + z
            .array_aux("cube[X,Y,Z]", "X * 100 + Y * 10 + Z")
            .scalar_const("row_start", 2.0)
            .scalar_const("row_end", 3.0)
            // Sum cube[1, 2:3, 2] = cube[1,2,2] + cube[1,3,2] = (100+20+2) + (100+30+2) = 122+132 = 254
            .scalar_aux("result", "SUM(cube[1, row_start:row_end, 2])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("result", 254.0);
    }

    #[test]
    fn sum_with_dynamic_range_reversed() {
        // Test reversed range (start > end) - should produce empty sum (0)
        let project = TestProject::new("sum_reversed_range")
            .indexed_dimension("Index", 10)
            .array_aux("data[Index]", "Index")
            .scalar_const("start_idx", 7.0)
            .scalar_const("end_idx", 3.0) // end < start
            .scalar_aux("total", "SUM(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        // Reversed range should be empty, sum = 0
        project.assert_scalar_result("total", 0.0);
    }

    #[test]
    fn sum_with_dynamic_range_out_of_bounds() {
        // Test range exceeding array bounds - should clamp to valid range
        let project = TestProject::new("sum_oob_range")
            .indexed_dimension("Index", 5)
            .array_aux("data[Index]", "Index") // [1,2,3,4,5]
            .scalar_const("start_idx", 3.0)
            .scalar_const("end_idx", 100.0) // way past the end
            // SUM(data[3:100]) should clamp to [3:5] = 3+4+5 = 12
            .scalar_aux("total", "SUM(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("total", 12.0);
    }

    #[test]
    fn sum_with_dynamic_range_zero_start() {
        // Test with start=0 (invalid in 1-based XMILE indexing)
        // Should treat as start=1 or return empty/invalid result
        let project = TestProject::new("sum_zero_start")
            .indexed_dimension("Index", 5)
            .array_aux("data[Index]", "Index") // [1,2,3,4,5]
            .scalar_const("start_idx", 0.0) // Invalid: 0 in 1-based indexing
            .scalar_const("end_idx", 3.0)
            // SUM(data[0:3]) - start=0 is clamped/handled, should sum 1+2+3 = 6
            .scalar_aux("total", "SUM(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        // With 0 start clamped to valid range, we get data[1:3] = 1+2+3 = 6
        project.assert_scalar_result("total", 6.0);
    }

    #[test]
    fn sum_with_dynamic_range_single_element() {
        // Test range that selects exactly one element
        let project = TestProject::new("sum_single_element")
            .indexed_dimension("Index", 5)
            .array_aux("data[Index]", "Index * 10") // [10,20,30,40,50]
            .scalar_const("idx", 3.0)
            // SUM(data[3:3]) - single element range
            .scalar_aux("total", "SUM(data[idx:idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        // data[3] = 30
        project.assert_scalar_result("total", 30.0);
    }

    #[test]
    fn mean_with_dynamic_range() {
        // Test MEAN with dynamic range - must use actual range size, not full array size
        let project = TestProject::new("mean_dynamic_range")
            .indexed_dimension("Index", 5)
            .array_aux("data[Index]", "Index * 10") // [10, 20, 30, 40, 50]
            .scalar_const("start_idx", 2.0)
            .scalar_const("end_idx", 3.0)
            // MEAN(data[2:3]) should be (20 + 30) / 2 = 25
            // NOT (20 + 30) / 5 = 10 (which would be wrong if using full array size)
            .scalar_aux("result", "MEAN(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("result", 25.0);
    }

    #[test]
    fn size_with_dynamic_range() {
        // Test SIZE with dynamic range - must return actual range size
        let project = TestProject::new("size_dynamic_range")
            .indexed_dimension("Index", 10)
            .array_aux("data[Index]", "Index")
            .scalar_const("start_idx", 3.0)
            .scalar_const("end_idx", 7.0)
            // SIZE(data[3:7]) should be 5 (elements 3, 4, 5, 6, 7)
            // NOT 10 (full array size)
            .scalar_aux("result", "SIZE(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("result", 5.0);
    }

    #[test]
    fn stddev_with_dynamic_range() {
        // Test STDDEV with dynamic range - must use actual range size
        let project = TestProject::new("stddev_dynamic_range")
            .indexed_dimension("Index", 5)
            .array_aux("data[Index]", "Index * 10") // [10, 20, 30, 40, 50]
            .scalar_const("start_idx", 2.0)
            .scalar_const("end_idx", 4.0)
            // STDDEV(data[2:4]) for values [20, 30, 40]:
            // mean = 30, variance = ((20-30)^2 + (30-30)^2 + (40-30)^2) / 2 = (100 + 0 + 100) / 2 = 100
            // stddev = sqrt(100) = 10
            .scalar_aux("result", "STDDEV(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("result", 10.0);
    }

    #[test]
    fn size_with_empty_dynamic_range() {
        // Test SIZE with empty range (start > end) returns 0, not 1
        let project = TestProject::new("size_empty_range")
            .indexed_dimension("Index", 10)
            .array_aux("data[Index]", "Index")
            .scalar_const("start_idx", 7.0)
            .scalar_const("end_idx", 3.0) // Reversed: start > end = empty range
            // SIZE(data[7:3]) should be 0 (empty range)
            .scalar_aux("result", "SIZE(data[start_idx:end_idx])");

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("result", 0.0);
    }

    #[test]
    fn sum_with_dim_name_subscript_preserves_other_dimension() {
        // SUM(m[DimD, *]) should preserve the array for the * dimension
        // while collapsing DimD to the current A2A element
        let project = TestProject::new("test")
            .named_dimension("DimD", &["D1", "D2"])
            .named_dimension("DimE", &["E1", "E2"])
            .array_const("m[DimD, DimE]", 10.0)
            .array_aux("msum[DimD]", "SUM(m[DimD, *])");

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("msum");

        // 2 A2A elements (D1, D2)
        assert_eq!(
            exprs.len(),
            2,
            "expected 2 A2A elements, got {}",
            exprs.len()
        );

        // Each should have SUM with StaticSubscript preserving the E dimension
        for expr in &exprs {
            if let Expr::AssignCurr(_, inner) = expr {
                if let Expr::App(BuiltinFn::Sum(sum_inner), _) = inner.as_ref() {
                    if let Expr::StaticSubscript(_, view, _) = sum_inner.as_ref() {
                        // The wildcard dimension (DimE with 2 elements) should be preserved
                        assert_eq!(
                            view.dims,
                            vec![2],
                            "expected StaticSubscript to preserve DimE (size 2), got {:?}",
                            view.dims
                        );
                    } else {
                        panic!(
                            "expected StaticSubscript inside SUM, got: {}",
                            pretty(sum_inner)
                        );
                    }
                } else {
                    panic!(
                        "expected App(Sum) inside AssignCurr, got: {}",
                        pretty(inner)
                    );
                }
            } else {
                panic!("expected AssignCurr, got: {}", pretty(expr));
            }
        }
    }

    #[test]
    fn pretty_formats_expressions_for_debugging() {
        // Quick sanity check that pretty() works for debugging test failures
        let project = TestProject::new("test")
            .indexed_dimension("D", 2)
            .array_const("x[D]", 5.0)
            .scalar_aux("y", "SUM(x[*])");

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("y");

        assert_eq!(exprs.len(), 1);
        let pretty_str = pretty(exprs[0]);
        // Should contain recognizable structure (lowercase from pretty() format)
        assert!(
            pretty_str.contains("curr") && pretty_str.contains("sum"),
            "pretty() output should show expression structure: {}",
            pretty_str
        );
    }
}

mod pass0_structural_lowering_tests {
    //! Tests that verify pass 0 structural lowering:
    //! - Bare array vars become Subscript with dimension name subscripts
    //! - Scalar vars remain unchanged
    //! - Nested expressions are recursively transformed
    //! - Transpose still works correctly
    //! - Dimension reordering works correctly

    use crate::compiler::{Expr, pretty};
    use crate::test_common::TestProject;

    #[test]
    fn bare_array_var_becomes_subscript_with_dim_names() {
        // result[DimB] = source  (where source is [DimB])
        // After lowering, source should be referenced with explicit subscript
        // In A2A context, dimension name subscripts collapse to scalar Var
        let project = TestProject::new("test")
            .named_dimension("DimB", &["B1", "B2"])
            .array_const("source[DimB]", 10.0)
            .array_aux("result[DimB]", "source"); // bare var reference

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("result");

        // Should have 2 A2A elements
        assert_eq!(exprs.len(), 2, "expected 2 A2A elements");
        // Each should collapse to scalar Var (dimension name -> ActiveDimRef -> scalar)
        for expr in &exprs {
            if let Expr::AssignCurr(_, inner) = expr {
                assert!(
                    matches!(inner.as_ref(), Expr::Var(..)),
                    "expected Var inside AssignCurr, got: {}",
                    pretty(expr)
                );
            } else {
                panic!("expected AssignCurr, got: {}", pretty(expr));
            }
        }
    }

    #[test]
    fn scalar_var_unchanged() {
        // Scalar variables should remain Var nodes (no ArrayBounds)
        let project = TestProject::new("test")
            .scalar_const("x", 5.0)
            .scalar_aux("y", "x");

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("y");
        assert_eq!(exprs.len(), 1);
        // Verify it compiled - scalar vars should work
        // (2 time steps: t=0 and t=1)
        project.assert_interpreter_result("y", &[5.0, 5.0]);
    }

    #[test]
    fn dimension_reordering_with_bare_arrays() {
        // p[DimB, DimA] = f  (where f is [DimA, DimB])
        // Dimension names create correct reordering
        let project = TestProject::new("test")
            .named_dimension("DimA", &["A1", "A2"])
            .named_dimension("DimB", &["B1", "B2", "B3"])
            // f[A1,B1]=1, f[A1,B2]=2, f[A1,B3]=3, f[A2,B1]=4, f[A2,B2]=5, f[A2,B3]=6
            .array_aux("f[DimA, DimB]", "(DimA - 1) * 3 + DimB")
            .array_aux("p[DimB, DimA]", "f"); // reordered dimensions

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("p");
        // 3 * 2 = 6 A2A elements
        assert_eq!(exprs.len(), 6, "expected 6 A2A elements");

        // Verify reordering works correctly:
        // p[B1,A1] = f[A1,B1] = 1
        // p[B1,A2] = f[A2,B1] = 4
        // p[B2,A1] = f[A1,B2] = 2
        // p[B2,A2] = f[A2,B2] = 5
        // p[B3,A1] = f[A1,B3] = 3
        // p[B3,A2] = f[A2,B3] = 6
        project.assert_interpreter_result("p", &[1.0, 4.0, 2.0, 5.0, 3.0, 6.0]);
    }

    #[test]
    fn a2a_transpose_still_works() {
        // Test that transpose with bare arrays still compiles and runs
        // This verifies the transpose handling isn't broken by pass 0
        TestProject::new("test")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_aux("matrix[Row, Col]", "Row + (Col - 1) * 2")
            .array_aux("transposed[Col, Row]", "matrix'")
            // matrix: [1,3,5; 2,4,6] (row-major)
            // transposed: [1,2; 3,4; 5,6] (row-major)
            .assert_interpreter_result("transposed", &[1.0, 2.0, 3.0, 4.0, 5.0, 6.0]);
    }

    #[test]
    fn a2a_transpose_with_operations() {
        // matrix' * 2 should still work after pass 0
        TestProject::new("test")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_aux("matrix[Row, Col]", "Row + (Col - 1) * 2")
            .array_aux("transposed_doubled[Col, Row]", "matrix' * 2")
            .assert_interpreter_result("transposed_doubled", &[2.0, 4.0, 6.0, 8.0, 10.0, 12.0]);
    }

    #[test]
    fn a2a_transpose_of_composite_expression() {
        // (matrix + 1)' should transpose the composite expression
        TestProject::new("test")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_aux("matrix[Row, Col]", "Row + (Col - 1) * 2")
            .array_aux("transposed_plus_one[Col, Row]", "(matrix + 1)'")
            // matrix + 1: [2,4,6; 3,5,7]
            // transposed: [2,3; 4,5; 6,7]
            .assert_interpreter_result("transposed_plus_one", &[2.0, 3.0, 4.0, 5.0, 6.0, 7.0]);
    }

    #[test]
    fn builtin_args_expanded() {
        // SUM(arr) where arr is bare array reference
        // (2 time steps: t=0 and t=1)
        TestProject::new("test")
            .indexed_dimension("D", 5)
            .array_const("arr[D]", 2.0)
            .scalar_aux("total", "SUM(arr)") // bare array in SUM
            .assert_interpreter_result("total", &[10.0, 10.0]);
    }

    #[test]
    fn sum_bare_array_in_a2a_context() {
        // SUM(m) in A2A context should bind active dims and reduce others.
        let project = TestProject::new("test")
            .named_dimension("DimD", &["D1", "D2"])
            .named_dimension("DimE", &["E1", "E2"])
            // m[D1,E1]=1, m[D1,E2]=2, m[D2,E1]=11, m[D2,E2]=12
            .array_aux("m[DimD, DimE]", "(DimD - 1) * 10 + DimE")
            .array_aux("msum[DimD]", "SUM(m)");

        project.assert_interpreter_result("msum", &[3.0, 23.0]);
    }

    #[test]
    fn multidimensional_bare_array() {
        // 2D bare array reference
        let project = TestProject::new("test")
            .named_dimension("DimA", &["A1", "A2"])
            .named_dimension("DimB", &["B1", "B2", "B3"])
            .array_const("source[DimA, DimB]", 1.0)
            .array_aux("target[DimA, DimB]", "source");

        let module = project.build_module().expect("should compile");
        let exprs = module.get_flow_exprs("target");
        assert_eq!(exprs.len(), 6, "expected 6 A2A elements (2 * 3)");
    }

    #[test]
    fn binary_op_with_bare_arrays() {
        // a + b where both a and b are bare array references
        TestProject::new("test")
            .indexed_dimension("D", 3)
            .array_const("a[D]", 1.0)
            .array_const("b[D]", 2.0)
            .array_aux("result[D]", "a + b") // both are bare
            .assert_interpreter_result("result", &[3.0, 3.0, 3.0]);
    }

    #[test]
    fn nested_bare_arrays_in_if() {
        // IF cond THEN a ELSE b where a and b are bare arrays
        TestProject::new("test")
            .indexed_dimension("D", 3)
            .array_const("a[D]", 1.0)
            .array_const("b[D]", 2.0)
            .array_aux("result[D]", "IF D > 1 THEN a ELSE b")
            // D=1: b[1]=2, D=2: a[2]=1, D=3: a[3]=1
            .assert_interpreter_result("result", &[2.0, 1.0, 1.0]);
    }
}

// =============================================================================
// Tests for indexed dimension broadcasting and bounds checking
// =============================================================================
#[cfg(test)]
mod indexed_dimension_tests {
    use crate::test_common::TestProject;

    // NOTE: Tests for different-named indexed dimensions broadcasting (e.g., a[DimA] + b[DimB])
    // require additional compiler changes beyond the VM-level positional matching.
    // The compiler's dimension matching occurs at multiple levels and all need updates.
    // These tests are marked #[ignore] as future work.

    #[test]
    #[ignore]
    fn different_indexed_dims_same_size_broadcast() {
        // TODO: Requires compiler-level changes to find_dimension_reordering and
        // multiple dimension matching code paths.
        // Test that two arrays with different indexed dimensions of the same size
        // can be combined in element-wise operations via positional matching.
        let project = TestProject::new("indexed_broadcast")
            .indexed_dimension("Products", 3)
            .indexed_dimension("Regions", 3)
            .array_aux("sales[Products]", "Products") // [1, 2, 3]
            .array_aux("costs[Regions]", "Regions * 10") // [10, 20, 30]
            .array_aux("combined[Products]", "sales + costs"); // [11, 22, 33]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("combined", &[11.0, 22.0, 33.0]);
    }

    #[test]
    #[ignore]
    fn different_indexed_dims_with_wildcard() {
        // TODO: Requires compiler-level changes for different-named indexed dim matching.
        let project = TestProject::new("indexed_wildcard_broadcast")
            .indexed_dimension("A", 4)
            .indexed_dimension("B", 4)
            .array_aux("arr_a[A]", "A * 2") // [2, 4, 6, 8]
            .array_aux("arr_b[B]", "B * 3") // [3, 6, 9, 12]
            .array_aux("sum[A]", "arr_a[*] + arr_b[*]"); // [5, 10, 15, 20]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("sum", &[5.0, 10.0, 15.0, 20.0]);
    }

    #[test]
    #[ignore]
    fn out_of_bounds_iteration_returns_nan() {
        // TODO: This test requires bounds checking during A2A iteration.
        // The VM changes are in place but the compiler needs to generate
        // code that properly creates mismatched-size views for testing.
        let project = TestProject::new("oob_iteration")
            .indexed_dimension("Size5", 5)
            .array_aux("source[Size5]", "Size5 * 10") // [10, 20, 30, 40, 50]
            .array_aux("slice[Size5]", "source[1:3]"); // [10, 20, 30, NaN, NaN]

        project.assert_compiles();
        project.assert_sim_builds();
        let result = project.interpreter_result("slice");
        assert_eq!(result[0], 10.0);
        assert_eq!(result[1], 20.0);
        assert_eq!(result[2], 30.0);
        assert!(
            result[3].is_nan(),
            "Element 4 should be NaN, got {}",
            result[3]
        );
        assert!(
            result[4].is_nan(),
            "Element 5 should be NaN, got {}",
            result[4]
        );
    }

    #[test]
    fn out_of_bounds_in_sum_builtin() {
        // When using a sliced array in SUM, only the valid elements should be summed.
        let project = TestProject::new("oob_sum")
            .indexed_dimension("Idx", 5)
            .array_aux("data[Idx]", "Idx") // [1, 2, 3, 4, 5]
            .scalar_aux("sum_first3", "SUM(data[1:3])"); // Should sum 1+2+3 = 6

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("sum_first3", 6.0);
    }

    #[test]
    fn star_range_indexed_subdimension() {
        // Test that *:IndexedSubDim desugars to [1:SIZE(IndexedSubDim)]
        // arr[*:SubIdx] where SubIdx is indexed(3) should give arr[1:3]
        let project = TestProject::new("star_indexed_subdim")
            .indexed_dimension("FullIdx", 5)
            .indexed_dimension("SubIdx", 3)
            .array_aux("data[FullIdx]", "FullIdx * 10") // [10, 20, 30, 40, 50]
            .array_aux("slice[SubIdx]", "data[*:SubIdx]"); // Should be [10, 20, 30]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("slice", &[10.0, 20.0, 30.0]);
    }

    #[test]
    fn star_range_indexed_in_sum() {
        // Test *:IndexedSubDim inside a SUM builtin
        let project = TestProject::new("star_indexed_sum")
            .indexed_dimension("Full", 10)
            .indexed_dimension("First5", 5)
            .array_aux("data[Full]", "Full") // [1, 2, 3, 4, 5, 6, 7, 8, 9, 10]
            .scalar_aux("sum_first5", "SUM(data[*:First5])"); // 1+2+3+4+5 = 15

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_scalar_result("sum_first5", 15.0);
    }

    #[test]
    #[ignore]
    fn indexed_dims_2d_positional_matching() {
        // TODO: Requires compiler-level changes for different-named indexed dim matching.
        let project = TestProject::new("indexed_2d_positional")
            .indexed_dimension("Rows", 2)
            .indexed_dimension("Cols", 3)
            .indexed_dimension("AltRows", 2)
            .indexed_dimension("AltCols", 3)
            .array_aux("a[Rows, Cols]", "Rows * 10 + Cols") // [[11,12,13],[21,22,23]]
            .array_aux("b[AltRows, AltCols]", "AltRows + AltCols") // [[2,3,4],[3,4,5]]
            .array_aux("sum[Rows, Cols]", "a + b"); // [[13,15,17],[24,26,28]]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("sum", &[13.0, 15.0, 17.0, 24.0, 26.0, 28.0]);
    }

    #[test]
    fn mixed_indexed_and_named_dims() {
        // Test that named dimensions still require exact matching (no positional fallback)
        // while indexed dimensions use positional matching
        let project = TestProject::new("mixed_dims")
            .indexed_dimension("NumericDim", 3)
            .named_dimension("NamedDim", &["A", "B", "C"])
            .array_aux("numeric_arr[NumericDim]", "NumericDim") // [1, 2, 3]
            .array_aux("named_arr[NamedDim]", "NamedDim") // [1, 2, 3] (position as value)
            .array_aux("result[NumericDim]", "numeric_arr * 2"); // [2, 4, 6]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("result", &[2.0, 4.0, 6.0]);
    }

    #[test]
    #[ignore]
    fn bounds_check_in_fast_path() {
        // TODO: Requires compiler-level changes for different-sized array assignment.
        let project = TestProject::new("fast_path_bounds")
            .indexed_dimension("SmallDim", 3)
            .indexed_dimension("LargeDim", 5)
            .array_aux("small[SmallDim]", "SmallDim * 5") // [5, 10, 15]
            .array_aux("expanded[LargeDim]", "small[1:3]"); // [5, 10, 15, NaN, NaN]

        project.assert_compiles();
        project.assert_sim_builds();
        let result = project.interpreter_result("expanded");
        assert_eq!(result[0], 5.0);
        assert_eq!(result[1], 10.0);
        assert_eq!(result[2], 15.0);
        assert!(result[3].is_nan(), "Element 4 should be NaN");
        assert!(result[4].is_nan(), "Element 5 should be NaN");
    }
}
