// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

package engine2

import (
	"context"
	"encoding/csv"
	"math"
	"os"
	"strconv"
	"strings"
	"testing"
)

func TestEngine(t *testing.T) {
	ctx := context.Background()

	// Create engine
	engine, err := NewEngine(ctx)
	if err != nil {
		t.Fatalf("failed to create engine: %v", err)
	}
	defer engine.Close()

	// Test error string
	errStr, err := engine.GetErrorString(ErrNoError)
	if err != nil {
		t.Fatalf("failed to get error string: %v", err)
	}
	if errStr != "no error" {
		t.Errorf("expected 'no error', got %q", errStr)
	}
}

func TestProject(t *testing.T) {
	ctx := context.Background()

	// Create engine
	engine, err := NewEngine(ctx)
	if err != nil {
		t.Fatalf("failed to create engine: %v", err)
	}
	defer engine.Close()

	// Load SIR Project protobuf fixture (simulatable)
	data, err := os.ReadFile("testdata/SIR_project.pb")
	if err != nil {
		t.Skipf("SIR_project.pb fixture not found: %v", err)
	}

	// Open project
	project, err := engine.OpenProject(data)
	if err != nil {
		t.Fatalf("failed to open project: %v", err)
	}
	defer project.Close()

	// Test reference counting
	err = project.Ref()
	if err != nil {
		t.Errorf("failed to increment ref count: %v", err)
	}

	err = project.Unref()
	if err != nil {
		t.Errorf("failed to decrement ref count: %v", err)
	}
}

func TestSimulation(t *testing.T) {
	ctx := context.Background()

	// Create engine
	engine, err := NewEngine(ctx)
	if err != nil {
		t.Fatalf("failed to create engine: %v", err)
	}
	defer engine.Close()

	// Load simulatable SIR Project
	data, err := os.ReadFile("testdata/SIR_project.pb")
	if err != nil {
		t.Skipf("SIR_project.pb fixture not found: %v", err)
	}

	// Open project
	project, err := engine.OpenProject(data)
	if err != nil {
		t.Fatalf("failed to open project: %v", err)
	}
	defer project.Close()

	// Create simulation
	sim, err := project.NewSim("")
	if err != nil {
		t.Fatalf("failed to create simulation: %v", err)
	}
	defer sim.Close()

	// Run simulation
	if err := sim.RunToEnd(); err != nil {
		t.Fatalf("failed to run SIR simulation: %v", err)
	}

	// Get step count
	stepCount, err := sim.GetStepCount()
	if err != nil {
		t.Fatalf("failed to get step count: %v", err)
	}
	if stepCount <= 0 {
		t.Errorf("expected positive step count, got %d", stepCount)
	}

	// Get variable count
	varCount, err := sim.GetVarCount()
	if err != nil {
		t.Fatalf("failed to get var count: %v", err)
	}
	if varCount <= 0 {
		t.Errorf("expected positive var count, got %d", varCount)
	}

	// Get variable names
	names, err := sim.GetVarNames()
	if err != nil {
		t.Fatalf("failed to get var names: %v", err)
	}
	if len(names) != varCount {
		t.Errorf("expected %d names, got %d", varCount, len(names))
	}
	t.Logf("var names: %v", names)

	// Validate series for a few key variables against expected CSV
	expectedPath := "testdata/SIR_output.csv"
	raw, err := os.ReadFile(expectedPath)
	if err != nil {
		t.Fatalf("missing expected output csv: %v", err)
	}
	// Normalize CR line endings to LF for the csv.Reader
	normalized := strings.ReplaceAll(string(raw), "\r", "\n")
	r := csv.NewReader(strings.NewReader(normalized))
	r.FieldsPerRecord = -1
	header, err := r.Read()
	if err != nil {
		t.Fatalf("failed reading header: %v", err)
	}
	// Map headers
	idx := map[string]int{}
	for i, h := range header {
		canon := strings.ToLower(strings.ReplaceAll(h, " ", "_"))
		idx[canon] = i
	}

	// Pull series from engine using engine variable names
	need := []string{"susceptible", "infectious", "recovered"}
	gotSeries := map[string][]float64{}
	// Build a lookup of canonicalized engine names
	engIdx := map[string]string{}
	for _, n := range names {
		canon := strings.ToLower(strings.ReplaceAll(n, " ", "_"))
		engIdx[canon] = n
	}
	for _, name := range need {
		engName, ok := engIdx[name]
		if !ok {
			t.Fatalf("engine variable %q not found in names", name)
		}
		series, err := sim.GetSeries(engName)
		if err != nil {
			t.Fatalf("series(%s): %v", engName, err)
		}
		gotSeries[name] = series
	}

	// Compare within tolerance
	row := 0
	for {
		rec, err := r.Read()
		if err != nil {
			break
		}
		if row >= stepCount {
			break
		}
		for _, name := range need {
			col, ok := idx[name]
			if !ok {
				t.Fatalf("expected column %s missing", name)
			}
			v, perr := strconv.ParseFloat(rec[col], 64)
			if perr != nil {
				t.Fatalf("parse csv: %v", perr)
			}
			if row >= len(gotSeries[name]) {
				t.Fatalf("engine series %s shorter than expected at row %d", name, row)
			}
			got := gotSeries[name][row]
			if !almostEq(v, got, 2e-3) {
				t.Fatalf("row %d %s: expected %.6g, got %.6g", row, name, v, got)
			}
		}
		row++
	}

	// Test GetValue after run: should return final value for a variable
	if len(gotSeries["infectious"]) == 0 {
		t.Fatalf("infectious series empty")
	}
	lastInfected := gotSeries["infectious"][len(gotSeries["infectious"])-1]
	engInfectious := engIdx["infectious"]
	gotVal, err := sim.GetValue(engInfectious)
	if err != nil {
		t.Fatalf("GetValue(infected) error: %v", err)
	}
	if !almostEq(lastInfected, gotVal, 2e-3) {
		t.Fatalf("GetValue mismatch: expected %.6g, got %.6g", lastInfected, gotVal)
	}

	// Interactive SetValue test on a fresh sim instance
	sim2, err := project.NewSim("")
	if err != nil {
		t.Fatalf("failed to create simulation2: %v", err)
	}
	defer sim2.Close()
	// Partially run, then set current value, then verify immediate get
	if err := sim2.RunTo(0.125); err != nil {
		t.Fatalf("RunTo(partial) error: %v", err)
	}
	// Set directly using the same name used in results (engInfectious)
	newVal := 42.0
	if err := sim2.SetValue(engInfectious, newVal); err != nil {
		t.Fatalf("SetValue (interactive) error: %v", err)
	}
	gotNow, err := sim2.GetValue(engInfectious)
	if err != nil {
		t.Fatalf("GetValue(after SetValue interactive) error: %v", err)
	}
	if !almostEq(newVal, gotNow, 1e-9) {
		t.Fatalf("interactive GetValue mismatch: expected %.9g, got %.9g", newVal, gotNow)
	}
}

func almostEq(a, b, eps float64) bool {
	if math.IsNaN(a) || math.IsNaN(b) {
		return false
	}
	if math.Abs(a) < 1e-9 && math.Abs(b) < 1e-9 {
		return true
	}
	return math.Abs(a-b) <= eps
}

func TestLTM(t *testing.T) {
	ctx := context.Background()

	// Create engine
	engine, err := NewEngine(ctx)
	if err != nil {
		t.Fatalf("failed to create engine: %v", err)
	}
	defer engine.Close()

	// Load simulatable SIR Project (expected to have feedback loops)
	data, err := os.ReadFile("testdata/SIR_project.pb")
	if err != nil {
		t.Skipf("SIR_project.pb fixture not found: %v", err)
	}

	// Open project
	project, err := engine.OpenProject(data)
	if err != nil {
		t.Fatalf("failed to open project: %v", err)
	}
	defer project.Close()

	// Enable LTM
	if err := project.EnableLTM(); err != nil {
		t.Skipf("EnableLTM not available: %v", err)
	}

	// Get loops
	loops, err := project.GetLoops()
	if err != nil {
		t.Skipf("GetLoops not available: %v", err)
	}
	if len(loops) == 0 {
		t.Skip("No loops detected in this model")
	}

	// Create simulation and run
	sim, err := project.NewSim("")
	if err != nil {
		t.Fatalf("failed to create sim: %v", err)
	}
	defer sim.Close()

	if err := sim.RunToEnd(); err != nil {
		t.Fatalf("failed to run sim: %v", err)
	}

	// Get loop score for first loop
	stepCount, err := sim.GetStepCount()
	if err != nil {
		t.Fatalf("GetStepCount: %v", err)
	}
	scores, err := sim.GetRelLoopScore(loops[0].ID)
	if err != nil {
		t.Fatalf("GetRelLoopScore: %v", err)
	}
	if len(scores) != stepCount {
		t.Fatalf("loop score length mismatch: got %d want %d", len(scores), stepCount)
	}
}
