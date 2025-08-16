// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

package engine2

import (
	"context"
	"math"
	"os"
	"testing"
)

// TestComprehensiveAPI tests all exported Go API functions
func TestComprehensiveAPI(t *testing.T) {
	ctx := context.Background()

	// Create engine
	engine, err := NewEngine(ctx)
	if err != nil {
		t.Fatalf("failed to create engine: %v", err)
	}
	defer engine.Close()

	// Load project
	data, err := os.ReadFile("testdata/SIR_project.pb")
	if err != nil {
		t.Skipf("SIR_project.pb fixture not found: %v", err)
	}

	project, err := engine.OpenProject(data)
	if err != nil {
		t.Fatalf("failed to open project: %v", err)
	}
	defer project.Close()

	// Test: Create simulation with empty model name
	t.Run("NewSim_EmptyModelName", func(t *testing.T) {
		sim, err := project.NewSim("")
		if err != nil {
			t.Fatalf("failed to create sim with empty model name: %v", err)
		}
		defer sim.Close()
	})

	// Test: Create simulation with specific model name
	t.Run("NewSim_SpecificModelName", func(t *testing.T) {
		sim, err := project.NewSim("main")
		if err != nil {
			// If "main" doesn't exist, that's ok for this test
			t.Logf("NewSim with 'main': %v", err)
		} else {
			defer sim.Close()
		}
	})

	// Test: RunTo partial simulation
	t.Run("RunTo_Partial", func(t *testing.T) {
		sim, err := project.NewSim("")
		if err != nil {
			t.Fatalf("failed to create sim: %v", err)
		}
		defer sim.Close()

		// Run to time 0.25
		err = sim.RunTo(0.25)
		if err != nil {
			t.Fatalf("RunTo(0.25) failed: %v", err)
		}

		// Get a value at this intermediate time
		names, err := sim.GetVarNames()
		if err != nil || len(names) == 0 {
			t.Fatalf("failed to get var names: %v", err)
		}

		val, err := sim.GetValue(names[0])
		if err != nil {
			t.Fatalf("GetValue at intermediate time failed: %v", err)
		}
		if math.IsNaN(val) || math.IsInf(val, 0) {
			t.Errorf("invalid value at intermediate time: %v", val)
		}

		// Continue running to another intermediate time
		err = sim.RunTo(0.5)
		if err != nil {
			t.Fatalf("RunTo(0.5) failed: %v", err)
		}

		// Run to end
		err = sim.RunToEnd()
		if err != nil {
			t.Fatalf("RunToEnd after partial runs failed: %v", err)
		}
	})

	// Test: Reset simulation
	t.Run("Reset", func(t *testing.T) {
		sim, err := project.NewSim("")
		if err != nil {
			t.Fatalf("failed to create sim: %v", err)
		}
		defer sim.Close()

		// Run partially
		err = sim.RunTo(0.5)
		if err != nil {
			t.Fatalf("initial RunTo failed: %v", err)
		}

		// Reset
		err = sim.Reset()
		if err != nil {
			t.Fatalf("Reset failed: %v", err)
		}

		// Run again after reset
		err = sim.RunToEnd()
		if err != nil {
			t.Fatalf("RunToEnd after reset failed: %v", err)
		}

		// Verify we have results
		stepCount, err := sim.GetStepCount()
		if err != nil {
			t.Fatalf("GetStepCount after reset and run failed: %v", err)
		}
		if stepCount <= 0 {
			t.Errorf("expected positive step count after reset and run, got %d", stepCount)
		}
	})

	// Test: Set and Get multiple values
	t.Run("SetGetMultipleValues", func(t *testing.T) {
		sim, err := project.NewSim("")
		if err != nil {
			t.Fatalf("failed to create sim: %v", err)
		}
		defer sim.Close()

		// Run partially
		err = sim.RunTo(0.125)
		if err != nil {
			t.Fatalf("RunTo failed: %v", err)
		}

		names, err := sim.GetVarNames()
		if err != nil || len(names) < 2 {
			t.Fatalf("need at least 2 variables for this test")
		}

		// Set multiple values
		testValues := []float64{10.0, 20.0}
		for i := 0; i < 2 && i < len(names); i++ {
			err = sim.SetValue(names[i], testValues[i])
			if err != nil {
				t.Fatalf("SetValue(%s, %f) failed: %v", names[i], testValues[i], err)
			}

			// Immediately verify the set value
			got, err := sim.GetValue(names[i])
			if err != nil {
				t.Fatalf("GetValue(%s) failed: %v", names[i], err)
			}
			if !almostEq(got, testValues[i], 1e-9) {
				t.Errorf("SetValue/GetValue mismatch for %s: set %f, got %f", names[i], testValues[i], got)
			}
		}
	})

	// Test: Error cases
	t.Run("ErrorCases", func(t *testing.T) {
		sim, err := project.NewSim("")
		if err != nil {
			t.Fatalf("failed to create sim: %v", err)
		}
		defer sim.Close()

		// Try to get value for non-existent variable
		_, err = sim.GetValue("this_variable_does_not_exist_12345")
		if err == nil {
			t.Error("expected error for non-existent variable, got nil")
		}

		// Try to set value for non-existent variable
		err = sim.SetValue("this_variable_does_not_exist_12345", 42.0)
		if err == nil {
			t.Error("expected error for setting non-existent variable, got nil")
		}

		// Try to get series for non-existent variable after running
		err = sim.RunToEnd()
		if err != nil {
			t.Fatalf("RunToEnd failed: %v", err)
		}

		_, err = sim.GetSeries("this_variable_does_not_exist_12345")
		if err == nil {
			t.Error("expected error for getting series of non-existent variable, got nil")
		}
	})

	// Test: Reference counting edge cases
	t.Run("ReferenceCountingEdgeCases", func(t *testing.T) {
		sim, err := project.NewSim("")
		if err != nil {
			t.Fatalf("failed to create sim: %v", err)
		}

		// Multiple refs and unrefs
		for i := 0; i < 3; i++ {
			err = sim.Ref()
			if err != nil {
				t.Errorf("Ref %d failed: %v", i, err)
			}
		}

		for i := 0; i < 3; i++ {
			err = sim.Unref()
			if err != nil {
				t.Errorf("Unref %d failed: %v", i, err)
			}
		}

		// Final close
		err = sim.Close()
		if err != nil {
			t.Errorf("Close failed: %v", err)
		}
	})

	// Test: LTM with multiple loops
	t.Run("LTM_MultipleLoops", func(t *testing.T) {
		// Enable LTM
		err := project.EnableLTM()
		if err != nil {
			t.Skipf("LTM not available: %v", err)
		}

		// Get loops
		loops, err := project.GetLoops()
		if err != nil {
			t.Fatalf("GetLoops failed: %v", err)
		}

		t.Logf("Found %d loops", len(loops))

		// Check each loop
		for i, loop := range loops {
			if loop.ID == "" {
				t.Errorf("Loop %d has empty ID", i)
			}
			if len(loop.Variables) == 0 {
				t.Errorf("Loop %d (%s) has no variables", i, loop.ID)
			}
			if loop.Polarity != LoopPolarityReinforcing && loop.Polarity != LoopPolarityBalancing {
				t.Errorf("Loop %d (%s) has invalid polarity: %d", i, loop.ID, loop.Polarity)
			}

			// Create sim and get loop scores
			sim, err := project.NewSim("")
			if err != nil {
				t.Fatalf("failed to create sim for loop testing: %v", err)
			}
			defer sim.Close()

			err = sim.RunToEnd()
			if err != nil {
				t.Fatalf("RunToEnd failed: %v", err)
			}

			scores, err := sim.GetRelLoopScore(loop.ID)
			if err != nil {
				t.Logf("GetRelLoopScore for loop %s failed: %v (may not be available for all loops)", loop.ID, err)
				continue
			}

			stepCount, _ := sim.GetStepCount()
			if len(scores) != stepCount {
				t.Errorf("Loop %s score length mismatch: got %d, want %d", loop.ID, len(scores), stepCount)
			}

			// Verify scores are valid numbers (NaN may be valid for initial timesteps)
			nanCount := 0
			for _, score := range scores {
				if math.IsNaN(score) {
					nanCount++
				} else if math.IsInf(score, 0) {
					t.Errorf("Loop %s has infinite score", loop.ID)
					break
				}
			}
			// It's ok if some scores are NaN (especially at the beginning)
			// but not all of them
			if nanCount == len(scores) && len(scores) > 1 {
				t.Errorf("Loop %s has all NaN scores", loop.ID)
			}
		}
	})

	// Test: Boundary conditions
	t.Run("BoundaryConditions", func(t *testing.T) {
		sim, err := project.NewSim("")
		if err != nil {
			t.Fatalf("failed to create sim: %v", err)
		}
		defer sim.Close()

		// Run to time 0 (should be no-op or succeed)
		err = sim.RunTo(0)
		if err != nil {
			t.Logf("RunTo(0) returned: %v", err)
		}

		// Run to negative time (should fail or be no-op)
		err = sim.RunTo(-1)
		if err == nil {
			t.Log("RunTo(-1) succeeded (implementation may allow it)")
		}

		// Run to very large time
		err = sim.RunTo(1e10)
		if err != nil {
			t.Logf("RunTo(1e10) failed as expected: %v", err)
		}
	})
}

// TestErrorStringComprehensive tests all error code strings
func TestErrorStringComprehensive(t *testing.T) {
	ctx := context.Background()
	engine, err := NewEngine(ctx)
	if err != nil {
		t.Fatalf("failed to create engine: %v", err)
	}
	defer engine.Close()

	// Test known error codes
	testCases := []struct {
		code     int32
		contains string // substring that should be in the error string
	}{
		{ErrNoError, "no_error"},
		{ErrDoesNotExist, "does_not_exist"},
		{ErrXmlDeserialization, "xml"},
		{ErrProtobufDecode, "protobuf"},
		{ErrCircularDependency, "circular"},
		{ErrNotSimulatable, "not_simulatable"},
		{ErrGeneric, "generic"},
		{-1, "unknown"}, // Test unknown error code
		{999, "unknown"}, // Test another unknown error code
	}

	for _, tc := range testCases {
		errStr, err := engine.GetErrorString(tc.code)
		if err != nil {
			t.Errorf("GetErrorString(%d) failed: %v", tc.code, err)
			continue
		}
		if errStr == "" {
			t.Errorf("GetErrorString(%d) returned empty string", tc.code)
		}
		// The error string should contain the expected substring or be "unknown_error" for unknown codes
		if tc.contains != "" && errStr != tc.contains && !contains(errStr, tc.contains) {
			t.Errorf("GetErrorString(%d) = %q, expected to contain %q", tc.code, errStr, tc.contains)
		}
	}
}

func contains(s, substr string) bool {
	if substr == "" {
		return true
	}
	for i := 0; i <= len(s)-len(substr); i++ {
		if s[i:i+len(substr)] == substr {
			return true
		}
	}
	return false
}