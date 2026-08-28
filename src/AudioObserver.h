#pragma once

#include <juce_audio_basics/juce_audio_basics.h>
#include <juce_audio_processors/juce_audio_processors.h>
#include <juce_core/juce_core.h>
#include <juce_dsp/juce_dsp.h>

#include <atomic>
#include <cstdint>
#include <functional>
#include <vector>

namespace apw
{

class AudioObserver final : private juce::Thread
{
public:
    using EventCallback = std::function<void (const juce::String& jsonEvent)>;

    AudioObserver();
    ~AudioObserver() override;

    void start (EventCallback callback);
    void stop();

    // Called from the real-time audio thread.
    void pushAudioBlock (const float* const* channelData, int numChannels, int numSamples);
    void pushMidiMessages (const juce::MidiBuffer& midi);
    void updateTransportState (juce::AudioPlayHead* playHead);
    void updateSessionConfig (int sampleRate, int channelCount, int bufferSize);
    void recordExternalAudioDrop (int numSamples) noexcept;

    // Thread-safe stats for the UI.
    int getWindowSize() const noexcept;
    std::uint64_t getTotalWindowsHashed() const noexcept;
    std::uint64_t getTotalEventsEmitted() const noexcept;
    std::uint64_t getBuffersSubmitted() const noexcept;
    std::uint64_t getSamplesSubmitted() const noexcept;
    std::uint64_t getFifoSamplesDropped() const noexcept;
    std::uint64_t getFifoWindowsDropped() const noexcept;
    juce::String getLastHash() const;

private:
    void run() override;
    void processWindow (const float* monoSamples, int numSamples);
    void drainMidiEvents();
    void checkTransportChanges();
    void checkSessionConfigChanges();

    juce::String computeChainedHash (const float* data, int numSamples);
    static double computeRMS (const float* data, int numSamples);
    static double computeZeroCrossingRate (const float* data, int numSamples);
    double computeSpectralCentroid (const float* data, int numSamples);

    // 3-band spectral profile: fraction of energy in low/mid/high bands.
    // Band boundaries (at 48 kHz): low 0-300 Hz, mid 300-4000 Hz, high 4000+ Hz.
    struct SpectralBands { double low = 0; double mid = 0; double high = 0; };
    SpectralBands computeSpectralBands (int numSamples);
    static constexpr double kBandShiftThreshold = 0.15;  // 15% energy ratio change

    // ── Audio FIFO (lock-free: audio thread writes, observer reads) ──
    static constexpr int kFFTOrder      = 12;
    static constexpr int kWindowSize    = 1 << kFFTOrder;            // 4096
    static constexpr int kFifoCapacity  = kWindowSize * 16;          // ~1.5 s at 44.1 kHz
    static constexpr int kMidiQueueSize = 256;
    static constexpr double kSilenceThreshold       = 0.001;         // RMS ~-60 dBFS
    static constexpr double kSpectralShiftThreshold  = 500.0;        // Hz

    juce::AbstractFifo audioFifo;
    std::vector<float> audioFifoBuffer;
    std::vector<float> windowBuffer;

    // ── MIDI FIFO ──
    struct MidiRecord
    {
        std::uint8_t type    = 0;
        std::uint8_t data1   = 0;
        std::uint8_t data2   = 0;
        std::uint8_t channel = 0;
    };
    juce::AbstractFifo midiFifo;
    std::vector<MidiRecord> midiFifoBuffer;

    // ── Transport (atomics: audio thread writes, observer reads) ──
    std::atomic<bool>       transportPlaying   { false };
    std::atomic<bool>       transportRecording { false };
    std::atomic<bool>       transportLooping   { false };
    std::atomic<juce::int64> transportSamplePos { -1 };
    std::atomic<int>        transportBpmX100   { 0 };

    bool prevTransportPlaying   = false;
    bool prevTransportRecording = false;
    bool prevTransportLooping   = false;
    int  prevTransportBpmX100   = 0;

    // ── Session config ──
    std::atomic<int> sessionSampleRate   { 44100 };
    std::atomic<int> sessionChannelCount { 2 };
    std::atomic<int> sessionBufferSize   { 512 };
    int prevSessionSampleRate   = 0;
    int prevSessionChannelCount = 0;

    // ── Hash chain ──
    juce::String previousHash;

    // ── Feature tracking ──
    bool   prevWindowHadAudio    = false;
    double prevSpectralCentroid  = 0.0;
    SpectralBands prevBands {};

    // ── CC parameter tracking (knob turn detection) ──
    struct CCState
    {
        int lastValue     = -1;
        int changeCount   = 0;
        std::uint64_t firstChangeMs = 0;
        std::uint64_t lastChangeMs  = 0;
    };
    static constexpr int kMaxCCTracked = 128;
    CCState ccStates[kMaxCCTracked] {};
    static constexpr int kKnobTurnMinChanges   = 3;
    static constexpr std::uint64_t kKnobTurnWindowMs = 500;

    // ── FFT ──
    juce::dsp::FFT fft;
    std::vector<float> fftWorkspace;

    // ── Stats ──
    std::atomic<std::uint64_t> totalWindowsHashed  { 0 };
    std::atomic<std::uint64_t> totalEventsEmitted  { 0 };
    std::atomic<std::uint64_t> buffersSubmitted { 0 };
    std::atomic<std::uint64_t> samplesSubmitted { 0 };
    std::atomic<std::uint64_t> fifoSamplesDropped { 0 };
    std::atomic<std::uint64_t> fifoWindowsDropped { 0 };
    mutable juce::SpinLock lastHashLock;
    juce::String lastHashHex;

    // ── Callback ──
    EventCallback eventCallback;

    JUCE_DECLARE_NON_COPYABLE_WITH_LEAK_DETECTOR (AudioObserver)
};

} // namespace apw
