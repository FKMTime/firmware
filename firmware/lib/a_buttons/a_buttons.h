#ifndef __A_BUTTONS_H__
#define __A_BUTTONS_H__

#include <Arduino.h>
#include <vector>

typedef void (*callback_t)();

struct ButtonCb {
    int callTime;
    bool called;
    bool afterUnpress;
    callback_t callback;
};

struct Button {
    uint8_t pin; // TODO: add multiple buttons press (at once)
    std::vector<ButtonCb> callbacks;
};

class AButtons {
  public:
    AButtons();
    size_t addButton(uint8_t _pin);
    void addButtonCb(size_t idx, int _callTime, bool _afterUnpress, callback_t callback);
    void loop();

  private:
    int checkDelay = 50;
    std::vector<Button> buttons;
};

#endif