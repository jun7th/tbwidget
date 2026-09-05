@echo off
setlocal

tar -czvf archive.tar.gz ^
  --exclude="node_modules" ^
  --exclude="src-tauri/target" ^
  --exclude=".git" ^
  --exclude=".vite" ^
  --exclude=".vscode" ^
  --exclude="dist" ^
  --exclude="save" ^
  --exclude="widget-history.sqlite" ^
  --exclude="archive.tar.gz" ^
  .

endlocal
