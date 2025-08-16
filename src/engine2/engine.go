// Copyright 2025 The Simlin Authors. All rights reserved.
// Use of this source code is governed by the Apache License,
// Version 2.0, that can be found in the LICENSE file.

package engine2

import (
	"context"
	_ "embed"
	"encoding/binary"
	"errors"
	"fmt"
	"math"
	"sync"

	"github.com/tetratelabs/wazero"
	"github.com/tetratelabs/wazero/api"
	"github.com/tetratelabs/wazero/imports/wasi_snapshot_preview1"
)

//go:embed engine2.wasm
var engineWasm []byte

// Error codes from the C API
const (
	ErrNoError        = 0
	ErrNoMem          = -1
	ErrBadFile        = -2
	ErrUnspecified    = -3
	ErrBadXml         = -4
	ErrBadLex         = -5
	ErrEof            = -6
	ErrCircular       = -7
	ErrNotSimulatable = -8
)

// LoopPolarity represents the polarity of a feedback loop
type LoopPolarity int

const (
	LoopPolarityReinforcing LoopPolarity = 0
	LoopPolarityBalancing   LoopPolarity = 1
)

// Loop represents a feedback loop in the model
type Loop struct {
	ID        string
	Variables []string
	Polarity  LoopPolarity
}

// Engine provides access to the simlin simulation engine via WebAssembly
type Engine struct {
	ctx context.Context
	rt  wazero.Runtime
	mod api.Module
	mu  sync.Mutex

	// Cached function references
	fnErrorStr               api.Function
	fnProjectOpen            api.Function
	fnProjectRef             api.Function
	fnProjectUnref           api.Function
	fnProjectEnableLTM       api.Function
	fnSimNew                 api.Function
	fnSimRef                 api.Function
	fnSimUnref               api.Function
	fnSimRunTo               api.Function
	fnSimRunToEnd            api.Function
	fnSimGetStepcount        api.Function
	fnSimGetVarcount         api.Function
	fnSimGetVarnames         api.Function
	fnSimReset               api.Function
	fnSimGetValue            api.Function
	fnSimSetValue            api.Function
	fnSimGetSeries           api.Function
	fnFreeString             api.Function
	fnAnalyzeGetLoops        api.Function
	fnFreeLoops              api.Function
	fnAnalyzeGetRelLoopScore api.Function

	// Memory management functions
	fnMalloc api.Function
	fnFree   api.Function
}

// NewEngine creates a new simulation engine instance
func NewEngine(ctx context.Context) (*Engine, error) {
	rt := wazero.NewRuntime(ctx)

	// Instantiate WASI
	wasi_snapshot_preview1.MustInstantiate(ctx, rt)

	// Compile the module
	compiled, err := rt.CompileModule(ctx, engineWasm)
	if err != nil {
		rt.Close(ctx)
		return nil, fmt.Errorf("failed to compile wasm module: %w", err)
	}

	// Instantiate the module
	mod, err := rt.InstantiateModule(ctx, compiled, wazero.NewModuleConfig())
	if err != nil {
		rt.Close(ctx)
		return nil, fmt.Errorf("failed to instantiate wasm module: %w", err)
	}

	e := &Engine{
		ctx: ctx,
		rt:  rt,
		mod: mod,
	}

	// Cache function references
	if err := e.cacheFunctions(); err != nil {
		e.Close()
		return nil, err
	}

	return e, nil
}

// cacheFunctions caches all the function references for better performance
func (e *Engine) cacheFunctions() error {
	functions := map[string]*api.Function{
		"simlin_error_str":                  &e.fnErrorStr,
		"simlin_project_open":               &e.fnProjectOpen,
		"simlin_project_ref":                &e.fnProjectRef,
		"simlin_project_unref":              &e.fnProjectUnref,
		"simlin_project_enable_ltm":         &e.fnProjectEnableLTM,
		"simlin_sim_new":                    &e.fnSimNew,
		"simlin_sim_ref":                    &e.fnSimRef,
		"simlin_sim_unref":                  &e.fnSimUnref,
		"simlin_sim_run_to":                 &e.fnSimRunTo,
		"simlin_sim_run_to_end":             &e.fnSimRunToEnd,
		"simlin_sim_get_stepcount":          &e.fnSimGetStepcount,
		"simlin_sim_get_varcount":           &e.fnSimGetVarcount,
		"simlin_sim_get_varnames":           &e.fnSimGetVarnames,
		"simlin_sim_reset":                  &e.fnSimReset,
		"simlin_sim_get_value":              &e.fnSimGetValue,
		"simlin_sim_set_value":              &e.fnSimSetValue,
		"simlin_sim_get_series":             &e.fnSimGetSeries,
		"simlin_free_string":                &e.fnFreeString,
		"simlin_analyze_get_loops":          &e.fnAnalyzeGetLoops,
		"simlin_free_loops":                 &e.fnFreeLoops,
		"simlin_analyze_get_rel_loop_score": &e.fnAnalyzeGetRelLoopScore,
		"simlin_malloc":                     &e.fnMalloc,
		"simlin_free":                       &e.fnFree,
	}

	for name, ref := range functions {
		fn := e.mod.ExportedFunction(name)
		if fn == nil {
			return fmt.Errorf("function %s not found in wasm module", name)
		}
		*ref = fn
	}

	return nil
}

// Close cleans up the engine resources
func (e *Engine) Close() error {
	if e.rt != nil {
		return e.rt.Close(e.ctx)
	}
	return nil
}

// errorString maps error codes to human-readable strings without calling into WASM.
// This avoids re-entrant locking and reduces overhead on error paths.
func (e *Engine) errorString(errCode int32) string {
	switch errCode {
	case ErrNoError:
		return "no error"
	case ErrNoMem:
		return "out of memory"
	case ErrBadFile:
		return "bad file"
	case ErrUnspecified:
		return "unspecified error"
	case ErrBadXml:
		return "bad XML"
	case ErrBadLex:
		return "lexer error"
	case ErrEof:
		return "unexpected end of file"
	case ErrCircular:
		return "circular dependency"
	case ErrNotSimulatable:
		return "not simulatable"
	default:
		return "unknown error"
	}
}

// Helper functions for memory management

func (e *Engine) malloc(size uint32) (uint32, error) {
	results, err := e.fnMalloc.Call(e.ctx, uint64(size))
	if err != nil {
		return 0, err
	}
	if len(results) != 1 {
		return 0, errors.New("malloc returned unexpected number of results")
	}
	return uint32(results[0]), nil
}

func (e *Engine) free(ptr uint32) error {
	_, err := e.fnFree.Call(e.ctx, uint64(ptr))
	return err
}

func (e *Engine) writeString(s string) (uint32, error) {
	data := []byte(s)
	// Add null terminator
	data = append(data, 0)

	ptr, err := e.malloc(uint32(len(data)))
	if err != nil {
		return 0, err
	}

	if !e.mod.Memory().Write(ptr, data) {
		e.free(ptr)
		return 0, errors.New("failed to write string to memory")
	}

	return ptr, nil
}

func (e *Engine) readString(ptr uint32) (string, error) {
	if ptr == 0 {
		return "", errors.New("null pointer")
	}

	// Read until null terminator
	var result []byte
	for i := uint32(0); ; i++ {
		b, ok := e.mod.Memory().ReadByte(ptr + i)
		if !ok {
			return "", errors.New("failed to read string from memory")
		}
		if b == 0 {
			break
		}
		result = append(result, b)
	}

	return string(result), nil
}

func (e *Engine) writeBytes(data []byte) (uint32, error) {
	ptr, err := e.malloc(uint32(len(data)))
	if err != nil {
		return 0, err
	}

	if !e.mod.Memory().Write(ptr, data) {
		e.free(ptr)
		return 0, errors.New("failed to write bytes to memory")
	}

	return ptr, nil
}

func (e *Engine) readFloat64Slice(ptr uint32, count int) ([]float64, error) {
	if ptr == 0 {
		return nil, errors.New("null pointer")
	}

	result := make([]float64, count)
	for i := 0; i < count; i++ {
		offset := ptr + uint32(i*8)
		bytes, ok := e.mod.Memory().Read(offset, 8)
		if !ok {
			return nil, fmt.Errorf("failed to read float64 at index %d", i)
		}
		bits := binary.LittleEndian.Uint64(bytes)
		result[i] = math.Float64frombits(bits)
	}

	return result, nil
}

func (e *Engine) writeFloat64Slice(data []float64) (uint32, error) {
	size := uint32(len(data) * 8)
	ptr, err := e.malloc(size)
	if err != nil {
		return 0, err
	}

	for i, v := range data {
		offset := ptr + uint32(i*8)
		bits := math.Float64bits(v)
		bytes := make([]byte, 8)
		binary.LittleEndian.PutUint64(bytes, bits)
		if !e.mod.Memory().Write(offset, bytes) {
			e.free(ptr)
			return 0, fmt.Errorf("failed to write float64 at index %d", i)
		}
	}

	return ptr, nil
}

// GetErrorString returns the string representation of an error code via WASM.
func (e *Engine) GetErrorString(errCode int32) (string, error) {
    e.mu.Lock()
    defer e.mu.Unlock()
    return e.getErrorStringLocked(errCode)
}

// getErrorStringLocked returns the string representation of an error code via WASM.
// Caller must hold e.mu.
func (e *Engine) getErrorStringLocked(errCode int32) (string, error) {
    // Call exported simlin_error_str(errCode) which returns a const char*
    res, err := e.fnErrorStr.Call(e.ctx, uint64(uint32(errCode)))
    if err != nil {
        return "", fmt.Errorf("simlin_error_str failed: %w", err)
    }
    if len(res) != 1 {
        return "", errors.New("simlin_error_str returned unexpected number of results")
    }
    ptr := uint32(res[0])
    s, rerr := e.readString(ptr)
    if rerr != nil {
        return "", fmt.Errorf("failed to read error string: %w", rerr)
    }
    return s, nil
}
