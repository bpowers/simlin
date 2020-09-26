#include "Main_Window.h"

#include <QFileDialog>
#include <QStandardPaths>
#include <boost/filesystem.hpp>

#include "Model.h"
#include "Vensim/VensimParse.h"
#include "ui_main_window.h"

static std::string QString_to_StdString(const QString &qs) {
  std::string s = qs.toUtf8().constData();
  return s;
}

static QString StdString_to_QString(const std::string &string) {
  return QString::fromUtf8(string.c_str());
}

Main_Window::Main_Window(QWidget *parent) : super(parent), ui(new Ui::Main_Window()) {
  ui->setupUi(this);

  this->connect(ui->filePicker, SIGNAL(clicked()), this, SLOT(choose_file()));
}

void Main_Window::choose_file() {
  if (ui->log->toPlainText().size() > 0)
    ui->log->append("");

  QString dir = QStandardPaths::locate(QStandardPaths::HomeLocation, "", QStandardPaths::LocateDirectory);
  QString value =
      QFileDialog::getOpenFileName(this, "Select an MDL file to conver to XMILE", dir, "Vensim Model(*.mdl)");
  if (value.isEmpty())
    return;

  Model m;
  VensimParse vp(&m);

  std::string file = QString_to_StdString(value);
  if (vp.ProcessFile(file)) {
    // mark variable types and potentially convert INTEG equations involving expressions
    // into flows (a single net flow on the first pass though this)
    m.MarkVariableTypes(NULL);

    // if there is a view then try to make sure everything is defined in the views
    // put unknowns in a heap in the first view at 20,20 but for things that have
    // connections try to put them in the right place
    m.AttachStragglers();

    boost::filesystem::path p(file);
    p.replace_extension(".xmile");

    std::vector<std::string> errs;
    m.WriteToXMILE(p.string(), errs);

    for (const std::string &err : errs) {
      ui->log->append(StdString_to_QString(err));
    }

    ui->log->append("Translation Complete: " + StdString_to_QString(p.string()));
  }
}
