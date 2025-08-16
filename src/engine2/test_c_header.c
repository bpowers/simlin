// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

// Test that the C header compiles correctly
#include "simlin.h"
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

int main() {
    // Test error string function
    const char *err_str = simlin_error_str(SIMLIN_ERR_NO_ERROR);
    printf("Error string for NO_ERROR: %s\n", err_str);
    
    // Test with invalid project data
    uint8_t dummy_data[] = {0x00, 0x01, 0x02};
    int err = 0;
    SimlinProject *project = simlin_project_open(dummy_data, sizeof(dummy_data), &err);
    
    if (project == NULL) {
        printf("Failed to open project as expected, error: %d\n", err);
        const char *err_msg = simlin_error_str(err);
        printf("Error message: %s\n", err_msg);
    } else {
        // Clean up if somehow succeeded
        simlin_project_unref(project);
    }
    
    // Test struct sizes are as expected
    printf("sizeof(SimlinLoop): %zu\n", sizeof(SimlinLoop));
    printf("sizeof(SimlinLoops): %zu\n", sizeof(SimlinLoops));
    printf("sizeof(SimlinErrorCode): %zu\n", sizeof(SimlinErrorCode));
    printf("sizeof(SimlinLoopPolarity): %zu\n", sizeof(SimlinLoopPolarity));
    
    // Verify enum values
    if (SIMLIN_ERR_NO_ERROR != 0) {
        printf("ERROR: SIMLIN_ERR_NO_ERROR should be 0\n");
        return 1;
    }
    
    if (SIMLIN_LOOP_REINFORCING != 0) {
        printf("ERROR: SIMLIN_LOOP_REINFORCING should be 0\n");
        return 1;
    }
    
    printf("C header test completed successfully!\n");
    return 0;
}