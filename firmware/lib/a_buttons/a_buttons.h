#ifndef __A_BUTTONS_H__
#define __A_BUTTONS_H__

#include <Arduino.h>
#include <vector>

struct Button;

typedef void (*callback_t)(Button &);
typedef void (*reoc_callback_t)(int);

struct ButtonCb {
  int callTime;
  bool called;
  bool afterRelease;
  callback_t callback;
  reoc_callback_t reocCallback;
};

struct Button {
  std::vector<uint8_t> pins;
  callback_t afterPressCb;
  callback_t afterReleaseCb;
  bool disableAfterReleaseCbs;
  std::vector<ButtonCb> callbacks;
};

class AButtons {
public:
  AButtons();
  size_t addButton(uint8_t _pin, callback_t _afterPressCb = NULL,
                   callback_t _afterReleaseCb = NULL);
  size_t addMultiButton(std::vector<uint8_t> _pins,
                        callback_t _afterPressCb = NULL,
                        callback_t _afterReleaseCb = NULL);
  void addButtonCb(size_t idx, int _callTime, bool _afterRelease,
                   callback_t callback);
  void addButtonReocCb(size_t idx, int _callInterval, reoc_callback_t callback);
  void testButtonClick(std::vector<uint8_t> pins, int pressTime);

  Button* getButton(size_t idx);
  int getLastHoldTime();

  /// @breif Loop function for button presses detection
  /// @return Index of pressed button event
  size_t loop();

private:
  int checkDelay = 15;
  std::vector<Button> buttons;

  int lastHoldTime = -1;
};

#endif
