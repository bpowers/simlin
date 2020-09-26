#ifndef __MAIN_WINDOW_H
#define __MAIN_WINDOW_H

#include <QMainWindow>

namespace Ui {
class Main_Window;
}

class Main_Window : public QMainWindow {
  Q_OBJECT

public:
  Main_Window(QWidget *parent = NULL);

protected slots:
  void choose_file();

private:
  typedef QMainWindow super;

  Ui::Main_Window *ui;
};

#endif
