#!/bin/bash

MODEL_DIR="$( cd "$( dirname "${BASH_SOURCE[0]}" )" && pwd )"
cd $MODEL_DIR
C_FILE=build/prune.c

function expect_present {
  grep -q "$1" $C_FILE
  if [[ $? != 0 ]]; then
    echo "ERROR: Did not find string '$1' that is expected to appear in $C_FILE"
    exit 1
  fi
}

function expect_not_present {
  grep -q "$1" $C_FILE
  if [[ $? != 1 ]]; then
    echo "ERROR: Found string '$1' that is expected to not appear in $C_FILE"
    exit 1
  fi
}

# Verify that used variables do appear in the generated C file
expect_present "_final_time"
expect_present "_initial_time"
expect_present "_saveper"
expect_present "_time_step"
expect_present "_input_1"
expect_present "_input_2"
expect_present "_a_values"
expect_present "_bc_values"
expect_present "_simple_1"
expect_present "_simple_2"
expect_present "_a_totals"
expect_present "_b1_totals"
expect_present "_input_1_and_2_total"
expect_present "_simple_totals"
expect_present "__lookup1"
expect_present "_look1"
expect_present "_look1_value_at_t1"
expect_present "_with_look1_at_t1"
expect_present "_constant_partial_1"
expect_present "_constant_partial_2"
expect_present "_initial_partial"
expect_present "_partial"
expect_present "_test_1_result = _IF_THEN_ELSE(_input_1 == 10.0, _test_1_t, _test_1_f);"
expect_present "_test_2_result = (_test_2_f);"
expect_present "_test_3_result = (_test_3_t);"
expect_present "_test_4_result = (_test_4_f);"
expect_present "_test_5_result = (_test_5_t);"
expect_present "_test_6_result = (_test_6_f);"
expect_present "_test_7_result = (_test_7_t);"
expect_present "_test_8_result = (_test_8_f);"
expect_present "_test_9_result = (_test_9_t);"
expect_present "_test_10_result = _IF_THEN_ELSE(_ABS(_test_10_cond), _test_10_t, _test_10_f);"
expect_present "_test_11_result = (_test_11_f);"
expect_present "_test_12_result = (_test_12_t);"
expect_present "_test_13_result = (_test_13_t1 + _test_13_t2) \* 10.0;"

# Verify that unreferenced variables do not appear in the generated C file
expect_not_present "_input_3"
expect_not_present "_d_values"
expect_not_present "_e_values"
expect_not_present "_e1_values"
expect_not_present "_e2_values"
expect_not_present "_d_totals"
expect_not_present "_input_2_and_3_total"
expect_not_present "__lookup2"
expect_not_present "_look2"
expect_not_present "_look2_value_at_t1"
expect_not_present "_with_look2_at_t1"
expect_not_present "_test_2_cond"
expect_not_present "_test_2_t"
expect_not_present "_test_3_cond"
expect_not_present "_test_3_f"
expect_not_present "_test_4_cond"
expect_not_present "_test_4_t"
expect_not_present "_test_5_cond"
expect_not_present "_test_5_f"
expect_not_present "_test_6_cond"
expect_not_present "_test_6_t"
expect_not_present "_test_7_cond"
expect_not_present "_test_7_f"
expect_not_present "_test_8_cond"
expect_not_present "_test_8_t"
expect_not_present "_test_9_cond"
expect_not_present "_test_9_f"
expect_not_present "_test_11_cond"
expect_not_present "_test_11_t"
expect_not_present "_test_12_cond"
expect_not_present "_test_12_f"
expect_not_present "_test_13_cond"
expect_not_present "_test_13_f"

echo "All validation checks passed!"
