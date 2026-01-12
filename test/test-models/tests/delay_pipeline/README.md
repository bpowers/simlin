Test Pipeline Delay
===========

This model tests pipeline delays. These have some funny interactions with the timestep, as they need to move everything from one bucket to the next each timestep.

For example, if the delay time is not an integer multiple of the step size, then vensim changes it, forcing it to be so!

![Vensim screenshot](vensim_screenshot.png)


Contributions
-------------

| Component                         | Author          | Contact                    | Date    | Software Version        |
|:--------------------------------- |:--------------- |:-------------------------- |:------- |:----------------------- |
| test_delays.mdl                   | James Houghton  | james.p.houghton@gmail.com | 10/10/17 | Vensim DSS 7.1a for Mac  |
| output.csv                        | James Houghton  | james.p.houghton@gmail.com | 10/10/17 | Vensim DSS 7.1a for Mac  |
