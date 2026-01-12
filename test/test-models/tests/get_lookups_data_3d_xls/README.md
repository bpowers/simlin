Test get lookups data 3d xls
============================

This model tests the data imported using GET XLS LOOKUPS and GET XLS DATA (XLS and DIRECT are identically implemented) for subscripted 3D matrix from an Excel file. Both of them are identically initialized in PySD. All the possible combinations from 0D to 3D are tested in the `unit_test_external.py`. This test aims to ensure the performance of the builder in the creation of the Python object, using 3D data to make sure it works well when the variable is defined in both one and several groups in the mdl file. The DATA is passed without arguments to make it compatible for PySD version with Python < 3.6, get data args 3d xls checks the data with arguments.
This test was later modified to be able to test the case when data is read from two different Excel files.

Contributions
-------------

| Component                           | Author          | Contact                         | Date    | Software Version                                      |
|:----------------------------------- |:--------------- |:------------------------------- |:-------- |:---------------------------------------------------- |
| `test_get_lookups_data_3d_xls.mdl`  | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
| `input.xls`                         | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
| `input2.xls`                        | Roger Samsó     | r.samso@proton.me               | 10/17/22 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
| `output.tab `                       | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
