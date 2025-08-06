// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

//! Focused tests for array functionality
//!
//! These tests are designed to incrementally test array features,
//! starting with compilation and then moving to interpreter execution.

#[cfg(test)]
mod wildcard_tests {
    use crate::array_test_helpers::ArrayTestProject;

    #[test]
    fn wildcard_preserves_dimension() {
        // Test that arr[*] preserves the array dimension
        let project = ArrayTestProject::new("wildcard_basic")
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
        let project = ArrayTestProject::new("wildcard_named")
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
        let project = ArrayTestProject::new("wildcard_arithmetic")
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
        let project = ArrayTestProject::new("wildcard_chained")
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
        let project = ArrayTestProject::new("wildcard_all_2d")
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
        let project = ArrayTestProject::new("wildcard_all_3d")
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
    #[ignore] // TODO: Implement partial wildcard support in multi-dimensional arrays
    fn wildcard_simple_2d() {
        // Simpler test for 2D arrays with wildcards
        let project = ArrayTestProject::new("wildcard_simple_2d")
            .indexed_dimension("A", 2)
            .indexed_dimension("B", 2)
            .array_const("m[A,B]", 5.0) // All elements = 5
            .array_aux("first_row[B]", "m[A.1, *]"); // Should be [5, 5]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("first_row", &[5.0, 5.0]);
    }

    #[test]
    #[ignore] // TODO: Implement partial wildcard support in multi-dimensional arrays
    fn wildcard_in_multidim_fixed_first() {
        // Test wildcard in multi-dimensional arrays with first dimension fixed
        let project = ArrayTestProject::new("wildcard_multidim_fixed_first")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("Row.1,Col.1", "10"),
                    ("Row.1,Col.2", "11"),
                    ("Row.1,Col.3", "12"),
                    ("Row.2,Col.1", "20"),
                    ("Row.2,Col.2", "21"),
                    ("Row.2,Col.3", "22"),
                ],
            )
            .array_aux("row1[Col]", "matrix[Row.1, *]") // Should be [10, 11, 12]
            .array_aux("row2[Col]", "matrix[Row.2, *]"); // Should be [20, 21, 22]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("row1", &[10.0, 11.0, 12.0]);
        project.assert_interpreter_result("row2", &[20.0, 21.0, 22.0]);
    }

    #[test]
    #[ignore] // TODO: Implement partial wildcard support in multi-dimensional arrays
    fn wildcard_in_multidim_fixed_second() {
        // Test wildcard in multi-dimensional arrays with second dimension fixed
        let project = ArrayTestProject::new("wildcard_multidim_fixed_second")
            .indexed_dimension("Row", 3)
            .indexed_dimension("Col", 2)
            .array_with_ranges(
                "matrix[Row,Col]",
                vec![
                    ("Row.1,Col.1", "110"),
                    ("Row.1,Col.2", "120"),
                    ("Row.2,Col.1", "210"),
                    ("Row.2,Col.2", "220"),
                    ("Row.3,Col.1", "310"),
                    ("Row.3,Col.2", "320"),
                ],
            )
            .array_aux("col1[Row]", "matrix[*, Col.1]") // Should be [110, 210, 310]
            .array_aux("col2[Row]", "matrix[*, Col.2]"); // Should be [120, 220, 320]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("col1", &[110.0, 210.0, 310.0]);
        project.assert_interpreter_result("col2", &[120.0, 220.0, 320.0]);
    }

    #[test]
    #[ignore] // TODO: Implement partial wildcard support in multi-dimensional arrays
    fn wildcard_with_named_and_indexed_dims() {
        // Test wildcard with mixed named and indexed dimensions
        let project = ArrayTestProject::new("wildcard_mixed_dims")
            .named_dimension("City", &["Boston", "NYC", "LA"])
            .indexed_dimension("Year", 2)
            .array_with_ranges(
                "population[City,Year]",
                vec![
                    ("City.Boston,Year.1", "1100"),
                    ("City.Boston,Year.2", "1200"),
                    ("City.NYC,Year.1", "1100"),
                    ("City.NYC,Year.2", "1200"),
                    ("City.LA,Year.1", "1100"),
                    ("City.LA,Year.2", "1200"),
                ],
            )
            .array_aux("boston_years[Year]", "population[City.Boston, *]") // Should be [1100, 1200]
            .array_aux("year1_cities[City]", "population[*, Year.1]"); // Should be [1100, 1100, 1100]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("boston_years", &[1100.0, 1200.0]);
        project.assert_interpreter_result("year1_cities", &[1100.0, 1100.0, 1100.0]);
    }

    #[test]
    #[ignore] // TODO: Implement partial wildcard support in multi-dimensional arrays
    fn wildcard_three_dimensions() {
        // Test wildcard in 3D arrays
        let project = ArrayTestProject::new("wildcard_3d")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .indexed_dimension("Z", 2)
            .array_with_ranges(
                "cube[X,Y,Z]",
                vec![
                    ("X.1,Y.1,Z.1", "111"),
                    ("X.1,Y.1,Z.2", "112"),
                    ("X.1,Y.2,Z.1", "121"),
                    ("X.1,Y.2,Z.2", "122"),
                    ("X.2,Y.1,Z.1", "211"),
                    ("X.2,Y.1,Z.2", "212"),
                    ("X.2,Y.2,Z.1", "221"),
                    ("X.2,Y.2,Z.2", "222"),
                ],
            )
            .array_aux("slice_xy[X,Y]", "cube[*,*,Z.1]") // Fix Z=1: [111, 121, 211, 221]
            .array_aux("slice_xz[X,Z]", "cube[*,Y.2,*]") // Fix Y=2: [121, 122, 221, 222]
            .array_aux("slice_yz[Y,Z]", "cube[X.1,*,*]"); // Fix X=1: [111, 112, 121, 122]

        project.assert_compiles();
        project.assert_sim_builds();
        project.assert_interpreter_result("slice_xy", &[111.0, 121.0, 211.0, 221.0]);
        project.assert_interpreter_result("slice_xz", &[121.0, 122.0, 221.0, 222.0]);
        project.assert_interpreter_result("slice_yz", &[111.0, 112.0, 121.0, 122.0]);
    }

    #[test]
    #[ignore] // TODO: Implement partial wildcard support in multi-dimensional arrays
    fn wildcard_multiple_in_expression() {
        // Test multiple wildcards in the same expression (both operands)
        let project = ArrayTestProject::new("wildcard_multiple")
            .indexed_dimension("Time", 3)
            .indexed_dimension("Product", 2)
            .array_with_ranges(
                "sales[Time,Product]",
                vec![
                    ("Time.1,Product.1", "10"), // (1) * 1 * 10 = 10
                    ("Time.1,Product.2", "20"), // (1) * 2 * 10 = 20
                    ("Time.2,Product.1", "20"), // (2) * 1 * 10 = 20
                    ("Time.2,Product.2", "40"), // (2) * 2 * 10 = 40
                    ("Time.3,Product.1", "30"), // (3) * 1 * 10 = 30
                    ("Time.3,Product.2", "60"), // (3) * 2 * 10 = 60
                ],
            )
            .array_with_ranges(
                "costs[Time,Product]",
                vec![
                    ("Time.1,Product.1", "5"),  // 1 * 5 = 5
                    ("Time.1,Product.2", "10"), // 2 * 5 = 10
                    ("Time.2,Product.1", "5"),  // 1 * 5 = 5
                    ("Time.2,Product.2", "10"), // 2 * 5 = 10
                    ("Time.3,Product.1", "5"),  // 1 * 5 = 5
                    ("Time.3,Product.2", "10"), // 2 * 5 = 10
                ],
            )
            .array_aux("profit[Time,Product]", "sales[*,*] - costs[*,*]"); // Element-wise subtraction

        project.assert_compiles();
        project.assert_sim_builds();
        // profit should be: [10-5, 20-10, 20-5, 40-10, 30-5, 60-10] = [5, 10, 15, 30, 25, 50]
        project.assert_interpreter_result("profit", &[5.0, 10.0, 15.0, 30.0, 25.0, 50.0]);
    }

    #[test]
    #[ignore] // Enable when wildcard is implemented
    fn wildcard_interpreter_basic() {
        ArrayTestProject::new("wildcard_interpreter")
            .indexed_dimension("Time", 3)
            .array_const("source[Time]", 10.0)
            .array_aux("result[Time]", "source[*]")
            .assert_interpreter_result("result", &[10.0, 10.0, 10.0]);
    }

    #[test]
    #[ignore] // Enable when wildcard is implemented
    fn wildcard_interpreter_expression() {
        ArrayTestProject::new("wildcard_expr")
            .indexed_dimension("Index", 3)
            .array_aux("values[Index]", "1 + Index") // Assuming Index gives position
            .array_aux("doubled[Index]", "values[*] * 2")
            .assert_interpreter_result("doubled", &[2.0, 4.0, 6.0]);
    }
}

#[cfg(test)]
mod dimension_position_tests {
    use crate::array_test_helpers::ArrayTestProject;
    use crate::common::ErrorCode;

    #[test]
    #[ignore]
    fn dimension_position_single() {
        // Test @1 syntax for accessing first dimension
        ArrayTestProject::new("dim_pos_single")
            .indexed_dimension("Time", 3)
            .array_const("arr[Time]", 5.0)
            .scalar_aux("first_elem", "arr[@1]")
            .assert_compile_error(ErrorCode::ArraysNotImplemented);
    }

    #[test]
    #[ignore]
    fn dimension_position_reorder() {
        // Test reordering dimensions with @2, @1
        ArrayTestProject::new("dim_pos_reorder")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_const("matrix[Row,Col]", 1.0)
            .array_aux("transposed[Col,Row]", "matrix[@2, @1]")
            .assert_compile_error(ErrorCode::ArraysNotImplemented);
    }

    #[test]
    #[ignore] // Enable when dimension position is implemented
    fn dimension_position_interpreter() {
        ArrayTestProject::new("dim_pos_interp")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 3)
            .array_aux("matrix[X,Y]", "X * 10 + Y")
            .array_aux("swapped[Y,X]", "matrix[@2, @1]")
            // matrix[0,0]=0, [0,1]=1, [0,2]=2, [1,0]=10, [1,1]=11, [1,2]=12
            // swapped[0,0]=matrix[0,0]=0, [0,1]=matrix[1,0]=10, etc.
            .assert_interpreter_result("swapped", &[0.0, 10.0, 1.0, 11.0, 2.0, 12.0]);
    }
}

#[cfg(test)]
mod transpose_tests {
    use crate::array_test_helpers::ArrayTestProject;
    use crate::common::ErrorCode;

    #[test]
    #[ignore]
    fn transpose_2d_array() {
        // Test transpose operator on 2D array
        ArrayTestProject::new("transpose_2d")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_const("matrix[Row,Col]", 5.0)
            .array_aux("transposed[Col,Row]", "matrix'")
            .assert_compile_error(ErrorCode::ArraysNotImplemented);
    }

    #[test]
    #[ignore]
    fn transpose_1d_array() {
        // Transpose of 1D array should be no-op
        ArrayTestProject::new("transpose_1d")
            .indexed_dimension("Time", 5)
            .array_const("vec[Time]", 3.0)
            .array_aux("result[Time]", "vec'")
            .assert_compile_error(ErrorCode::ArraysNotImplemented);
    }

    #[test]
    #[ignore] // Enable when transpose is implemented
    fn transpose_interpreter_basic() {
        ArrayTestProject::new("transpose_interp")
            .indexed_dimension("Row", 2)
            .indexed_dimension("Col", 3)
            .array_aux("matrix[Row,Col]", "Row * 3 + Col")
            .array_aux("transposed[Col,Row]", "matrix'")
            // matrix: [[0,1,2],[3,4,5]]
            // transposed: [[0,3],[1,4],[2,5]]
            .assert_interpreter_result("transposed", &[0.0, 3.0, 1.0, 4.0, 2.0, 5.0]);
    }

    #[test]
    #[ignore] // Enable when transpose is implemented
    fn transpose_chain() {
        // Test that (A')' = A
        ArrayTestProject::new("transpose_chain")
            .indexed_dimension("X", 2)
            .indexed_dimension("Y", 2)
            .array_aux("original[X,Y]", "X + Y * 10")
            .array_aux("double_transpose[X,Y]", "(original')'")
            .assert_interpreter_result("double_transpose", &[0.0, 10.0, 1.0, 11.0]);
    }
}

#[cfg(test)]
mod range_tests {
    use crate::array_test_helpers::ArrayTestProject;
    use crate::common::ErrorCode;

    #[test]
    #[ignore]
    fn range_basic() {
        // Test basic range subscript [1:3]
        ArrayTestProject::new("range_basic")
            .indexed_dimension("Time", 5)
            .array_aux("source[Time]", "Time")
            .array_aux("slice[Time]", "source[1:3]") // Should create smaller array
            .assert_compile_error(ErrorCode::TodoRange);
    }

    #[test]
    #[ignore]
    fn range_with_expressions() {
        // Test range with expressions [start:end]
        ArrayTestProject::new("range_expr")
            .indexed_dimension("Index", 10)
            .scalar_const("start", 2.0)
            .scalar_const("end", 5.0)
            .array_const("data[Index]", 1.0)
            .array_aux("slice[Index]", "data[start:end]")
            .assert_compile_error(ErrorCode::TodoRange);
    }

    #[test]
    #[ignore]
    fn range_open_ended() {
        // Test open-ended ranges [:3] and [2:]
        ArrayTestProject::new("range_open")
            .indexed_dimension("Time", 5)
            .array_const("arr[Time]", 10.0)
            .array_aux("prefix[Time]", "arr[:3]")
            .array_aux("suffix[Time]", "arr[2:]")
            .assert_compile_error(ErrorCode::TodoRange);
    }

    #[test]
    #[ignore] // Enable when ranges are implemented
    fn range_interpreter_basic() {
        ArrayTestProject::new("range_interp")
            .indexed_dimension("Index", 5)
            .array_aux("source[Index]", "Index * 10") // [0, 10, 20, 30, 40]
            .array_aux("middle", "source[1:4]") // Should be [10, 20, 30]
            .assert_interpreter_result("middle", &[10.0, 20.0, 30.0]);
    }

    #[test]
    #[ignore] // Enable when ranges are implemented
    fn range_multidim() {
        ArrayTestProject::new("range_multidim")
            .indexed_dimension("Row", 4)
            .indexed_dimension("Col", 4)
            .array_aux("matrix[Row,Col]", "Row * 10 + Col")
            .array_aux("submatrix", "matrix[1:3, 0:2]") // 2x2 submatrix
            .assert_interpreter_result("submatrix", &[10.0, 11.0, 20.0, 21.0]);
    }
}

#[cfg(test)]
mod star_range_tests {
    use crate::array_test_helpers::ArrayTestProject;
    use crate::common::ErrorCode;

    #[test]
    #[ignore]
    fn star_range_to_end() {
        // Test *:DimName syntax
        ArrayTestProject::new("star_range")
            .named_dimension("City", &["Boston", "NYC", "LA", "SF"])
            .array_const("population[City]", 1000000.0)
            .array_aux("west_coast[City]", "population[*:City.LA]")
            .assert_compile_error(ErrorCode::TodoStarRange);
    }

    #[test]
    #[ignore] // Enable when star ranges are implemented
    fn star_range_interpreter() {
        ArrayTestProject::new("star_range_interp")
            .named_dimension("Month", &["Jan", "Feb", "Mar", "Apr"])
            .array_aux("sales[Month]", "Month * 100") // Assuming Month gives index
            .array_aux("q1_sales", "sales[*:Month.Mar]")
            .assert_interpreter_result("q1_sales", &[0.0, 100.0, 200.0]);
    }
}

#[cfg(test)]
mod combined_operations_tests {
    use crate::array_test_helpers::ArrayTestProject;

    #[test]
    #[ignore] // Enable when all operations are implemented
    fn transpose_and_slice() {
        // Combine transpose with slicing
        ArrayTestProject::new("combined_transpose_slice")
            .indexed_dimension("Row", 3)
            .indexed_dimension("Col", 4)
            .array_aux("matrix[Row,Col]", "Row * 10 + Col")
            .array_aux("result", "matrix'[1:3, *]") // Transpose then slice
            .assert_interpreter_result("result", &[1.0, 11.0, 21.0, 2.0, 12.0, 22.0]);
    }

    #[test]
    #[ignore] // Enable when all operations are implemented
    fn dimension_position_and_wildcard() {
        // Combine dimension position with wildcard
        ArrayTestProject::new("combined_dimpos_wildcard")
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
        ArrayTestProject::new("complex_expr")
            .indexed_dimension("Time", 5)
            .indexed_dimension("Product", 3)
            .array_aux("sales[Time,Product]", "Time * Product")
            .array_aux("costs[Time,Product]", "Product * 10")
            .array_aux("profit[Time,Product]", "sales[*,*] - costs[*,*]")
            .array_aux("total_profit[Time]", "SUM(profit[*, Product.*])")
            .assert_compiles();
    }
}

#[cfg(test)]
mod error_handling_tests {
    #[test]
    #[ignore] // Enable when dimension checking is fully implemented
    fn dimension_mismatch() {
        // Test that dimension mismatches are caught
        // ArrayTestProject::new("dim_mismatch")
        //     .indexed_dimension("X", 3)
        //     .indexed_dimension("Y", 4)
        //     .array_const("arr1[X]", 1.0)
        //     .array_const("arr2[Y]", 2.0)
        //     .array_aux("result[X]", "arr1[*] + arr2[*]")  // Should fail - different dimensions
        //     .assert_compile_error(ErrorCode::ArrayDimensionMismatch);
    }

    #[test]
    #[ignore] // Enable when bounds checking is implemented
    fn out_of_bounds_index() {
        // Test out of bounds access
        // ArrayTestProject::new("out_of_bounds")
        //     .indexed_dimension("Small", 3)
        //     .array_const("arr[Small]", 10.0)
        //     .scalar_aux("bad_access", "arr[5]")  // Index 5 out of bounds for size 3
        //     .assert_compile_error(ErrorCode::ArrayIndexOutOfBounds);
    }
}
