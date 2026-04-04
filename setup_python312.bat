@echo off
set PY=C:\Users\kushn\AppData\Local\Programs\Python\Python312\python.exe

echo Testing Python...
%PY% --version

echo Installing pip...
%PY% C:\tmp\get-pip.py

echo Installing ML packages...
%PY% -m pip install pandas xgboost scikit-learn numpy

echo Testing imports...
%PY% -c "import pandas; import xgboost; import sklearn; print('ALL OK')"

pause
