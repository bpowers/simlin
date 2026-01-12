Test get subscript 3d arrays xls
============================

This model tests the data imported using GET DIRECT CONSTANTS and GET DIRECT SUBSCRIPT (XLS and DIRECT are identically implemented) for subscripted 3D matrix from an Excel file. All the possible combinations from 0D to 3D are tested in the `unit_test_external.py`. This test aims to ensure the performance of the builder in the creation of the Python object, using 3D data to make sure it works well when the variable is defined in both one and several groups in the mdl file.


Contributions
-------------

| Component                              | Author          | Contact                         | Date    | Software Version                                      |
|:-------------------------------------- |:--------------- |:------------------------------- |:-------- |:---------------------------------------------------- |
| `test_get_subscript_3d_arrays_xls.mdl` | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
| `input.xls`                            | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
| `output.tab `                          | Eneko Martin    | eneko.martin.martinez@gmail.com | 11/18/20 | Vensim DSS for Windows 7.3.4 single precision (x32)  |
