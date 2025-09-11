#!/usr/bin/env python3
"""Context manager usage examples for the simlin Python package.

This module demonstrates how to use context managers with Project, Model, and Sim classes
for automatic resource cleanup when working with system dynamics models.
"""

import simlin
import pandas as pd
import numpy as np
from pathlib import Path


def basic_context_manager_usage():
    """Example: Basic context manager usage with automatic cleanup."""
    print("=" * 60)
    print("Example 1: Basic Context Manager Usage")
    print("=" * 60)
    
    # Load a model using context manager - automatic cleanup when done
    model_path = Path("../tests/fixtures/eval_order.stmx")
    
    with simlin.Project.from_file(model_path) as project:
        print(f"Number of models: {project.get_model_count()}")
        print(f"Model names: {project.get_model_names()}")
        
        # Get the default model
        with project.get_model() as model:
            print(f"Number of variables: {len(model.get_var_names())}")
            
            # Create and run a simulation with context manager
            with model.new_sim() as sim:
                sim.run_to_end()
                print(f"Simulation completed with {sim.get_step_count()} time steps")
                
                # Get results as DataFrame
                results = sim.get_results()
                print(f"Results shape: {results.shape}")
                
    # All resources automatically cleaned up when exiting 'with' blocks
    print("All resources have been automatically cleaned up!")


def exception_handling_example():
    """Example: Context managers with exception handling."""
    print("\n" + "=" * 60)
    print("Example 2: Exception Handling with Context Managers")
    print("=" * 60)
    
    model_path = Path("../tests/fixtures/eval_order.stmx")
    
    try:
        with simlin.Project.from_file(model_path) as project:
            with project.get_model() as model:
                with model.new_sim() as sim:
                    # Simulate an error condition
                    print("Starting simulation...")
                    sim.run_to_end()
                    
                    # This would normally cause an error, but let's simulate one
                    if sim.get_step_count() > 0:
                        print("Simulating an error...")
                        raise ValueError("Simulated error during analysis")
                        
    except ValueError as e:
        print(f"Caught expected error: {e}")
        print("Resources were still cleaned up automatically!")
    
    print("Exception handling complete - resources cleaned up")


def mixed_usage_patterns():
    """Example: Mixing context managers with traditional usage."""
    print("\n" + "=" * 60)
    print("Example 3: Mixed Usage Patterns")
    print("=" * 60)
    
    model_path = Path("../tests/fixtures/eval_order.stmx")
    
    # Traditional usage (still works exactly as before)
    print("Traditional usage (no context manager):")
    project1 = simlin.Project.from_file(model_path)
    model1 = project1.get_model()
    sim1 = model1.new_sim()
    sim1.run_to_end()
    print(f"Traditional sim completed with {sim1.get_step_count()} steps")
    # Cleanup happens automatically via finalizers
    
    # Context manager usage for explicit control
    print("\nContext manager usage (explicit cleanup):")
    with simlin.Project.from_file(model_path) as project2:
        with project2.get_model() as model2:
            with model2.new_sim() as sim2:
                sim2.run_to_end()
                print(f"Context manager sim completed with {sim2.get_step_count()} steps")
            # Explicit cleanup happens here
    
    # Both patterns can be mixed as needed
    print("Both usage patterns work seamlessly together!")


def advanced_analysis_with_context_managers():
    """Example: Advanced analysis using context managers."""
    print("\n" + "=" * 60)
    print("Example 4: Advanced Analysis with Context Managers")
    print("=" * 60)
    
    model_path = Path("../tests/fixtures/eval_order.stmx")
    
    # Comprehensive analysis with automatic cleanup
    with simlin.Project.from_file(model_path) as project:
        # Analyze project-level information
        print("Project Analysis:")
        loops = project.get_loops()
        print(f"Found {len(loops)} feedback loops")
        
        errors = project.get_errors()
        if errors:
            print(f"Found {len(errors)} errors/warnings")
        else:
            print("No errors found in project")
        
        with project.get_model() as model:
            # Analyze model structure
            print(f"\nModel Analysis:")
            var_names = model.get_var_names()
            print(f"Variables: {len(var_names)}")
            
            # Analyze causal links
            links = model.get_links()
            print(f"Causal links: {len(links)}")
            
            # Run simulation with LTM for detailed analysis
            with model.new_sim(enable_ltm=True) as sim:
                print(f"\nRunning simulation with LTM analysis...")
                sim.run_to_end()
                
                # Get comprehensive results
                results = sim.get_results()
                print(f"Results DataFrame shape: {results.shape}")
                
                # Analyze links with scores
                sim_links = sim.get_links()
                scored_links = [link for link in sim_links if link.has_score()]
                print(f"Links with LTM scores: {len(scored_links)}")
                
                # Show some statistics
                if len(results) > 0:
                    print(f"\nSimulation Statistics:")
                    print(f"Time range: {results.index.min():.2f} to {results.index.max():.2f}")
                    if len(results.columns) > 0:
                        first_var = results.columns[0]
                        print(f"Sample variable '{first_var}': "
                              f"min={results[first_var].min():.3f}, "
                              f"max={results[first_var].max():.3f}")
    
    print("Advanced analysis complete - all resources cleaned up!")


def context_manager_benefits():
    """Example: Demonstrating the benefits of context managers."""
    print("\n" + "=" * 60)
    print("Example 5: Benefits of Context Manager Usage")
    print("=" * 60)
    
    print("Context managers provide several benefits:")
    print("1. Automatic resource cleanup")
    print("2. Exception safety")
    print("3. Clear resource lifetime")
    print("4. No memory leaks")
    print("5. Clean, readable code structure")
    print()
    
    model_path = Path("../tests/fixtures/eval_order.stmx")
    
    # Demonstrate resource management
    print("Running multiple simulations with automatic cleanup:")
    
    for i in range(3):
        with simlin.Project.from_file(model_path) as project:
            with project.get_model() as model:
                with model.new_sim() as sim:
                    sim.run_to_end()
                    steps = sim.get_step_count()
                    print(f"  Simulation {i+1}: {steps} steps completed")
                # Resources automatically cleaned up after each iteration
    
    print("All simulations completed with automatic cleanup!")
    print("\nKey advantages:")
    print("- No need to manually manage C library resources")
    print("- Guaranteed cleanup even if exceptions occur")
    print("- Clear scope of resource usage")
    print("- Optional - existing code continues to work unchanged")


if __name__ == "__main__":
    """Run all context manager examples."""
    print("Context Manager Usage Examples for PySilin")
    print("=" * 80)
    
    try:
        basic_context_manager_usage()
        exception_handling_example()
        mixed_usage_patterns()
        advanced_analysis_with_context_managers()
        context_manager_benefits()
        
        print("\n" + "=" * 80)
        print("All examples completed successfully!")
        print("Context managers provide safe, clean resource management.")
        
    except FileNotFoundError:
        print("\nNote: Some examples require test fixture files.")
        print("Run from the pysimlin package directory or adjust paths.")
    except Exception as e:
        print(f"\nUnexpected error: {e}")
        print("This demonstrates that exceptions are handled gracefully!")