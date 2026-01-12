# Created by houghton at 9/26/17
Feature: Extreme Conditions Tests
  # Enter feature description here

  Scenario: Hare Oblivion
    Given the model 'Lotka_Volterra.mdl'
    When Prey is set to 0
    Then Prey Births is immediately equal to 0
    And Prey Deaths is immediately equal to 0
    When the model is run
    Then Predators is equal to 0 at time 12

  Scenario: Foxes Loose Appetite
    # Prey should still have some mortality, even if they
    # aren't being eaten.
    Given the model 'Lotka_Volterra.mdl'
    When Reference Fractional Predation Rate is set to 0
    Then Prey Deaths is greater than 0 at time 12
    