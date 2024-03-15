#ifndef __STACKMAT_H__
#define __STACKMAT_H__

#define STACKMAT_TIMER_BAUD_RATE 1200
#define STACKMAT_TIMER_TIMEOUT 1000

enum StackmatTimerState {
  ST_Unknown = 0,
  ST_Reset = 'I',
  ST_Running = ' ',
  ST_Stopped = 'S'
};

class Stackmat {
  public:
    Stackmat();
    void begin(Stream *_serial);
    void loop();

    uint8_t displayMinutes();
    uint8_t displaySeconds();
    uint16_t displayMilliseconds();
    
    bool connected();
    StackmatTimerState state();
    int time();

  private:
    StackmatTimerState currentTimerState = ST_Reset;
    unsigned long lastUpdated = 0;
    int timerTime = 0;
    Stream *serial;

    String ReadStackmatString();
    bool ParseTimerData(String data);
};

#endif