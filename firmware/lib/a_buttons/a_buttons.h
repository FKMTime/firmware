#ifndef __A_BUTTONS_H__
#define __A_BUTTONS_H__

#include <Arduino.h>
#include <vector>

typedef void (*callback_t)();
typedef void (*reoc_callback_t)(int);

struct ButtonCb {
    int callTime;
    bool called;
    bool afterRelease;
    callback_t callback;
    reoc_callback_t reocCallback;
};

struct Button {
    uint8_t pin; // TODO: add multiple buttons press (at once)
    callback_t afterReleaseCb;
    std::vector<ButtonCb> callbacks;
};

class AButtons {
  public:
    AButtons();
    size_t addButton(uint8_t _pin, callback_t _afterReleaseCb = NULL);
    void addButtonCb(size_t idx, int _callTime, bool _afterRelease, callback_t callback);
    void addButtonReocCb(size_t idx, int _callInterval, reoc_callback_t callback);
    void loop();

  private:
    int checkDelay = 50;
    std::vector<Button> buttons;
};

#endif