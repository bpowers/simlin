#!/bin/bash

#ARCHITECT="/Applications/Stella Architect.app"
#XMUTIL="XMUtil"
XMUTIL="/Users/bschoenberg/code/personal/xmutil/DerivedData/XMUtil/Build/Products/Debug/XMUtil"
ARCHITECT="/Users/bschoenberg/code/dev_branch/StellaQt/DerivedData/StellaQt/Build/Products/Debug/Stella Architect.app"
DIR="./tests"

for input_dir in $DIR
do
	echo "Generate xmile files into: $input_dir"

	find $input_dir -name '*.mdl' | sed 's/\.mdl$//' | while read f; do

	 	echo "Converting $f.mdl"
	    "$XMUTIL" "$f.mdl"
	    if [ -f "$f.xmile" ]; then
	    	cp "$f.xmile" "$f.stmx"

	    	CSV_NAME="$(dirname $f)/output_stella.csv"
	    	PNG_NAME="$(dirname $f)/stella_screenshot.png"

	    	echo "Opening in Architect $f.stmx"
	    	open -W -a "$ARCHITECT" "$f.stmx" --args -ssm $PWD/$PNG_NAME -r -xall $PWD/$CSV_NAME  -s -q 
	    else
	    	echo "XMUtil hit an error"
	    	sleep 10
	    fi
	done
done