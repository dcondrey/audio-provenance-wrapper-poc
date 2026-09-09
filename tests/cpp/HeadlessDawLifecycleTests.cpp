#include "PluginProcessor.h"

#include <juce_audio_utils/juce_audio_utils.h>

#include <algorithm>
#include <atomic>
#include <chrono>
#include <cmath>
#include <cstdint>
#include <cstdlib>
#include <cstring>
#include <iostream>
#include <limits>
#include <memory>
#include <new>
#include <string>
#include <thread>

#if defined (__APPLE__)
 #include <malloc/malloc.h>
#elif defined (__linux__)
 #include <malloc.h>
#endif

namespace
{
std::atomic<std::uint64_t> liveAllocatedBytes { 0 };
thread_local bool countRealtimeAllocations = false;
thread_local std::uint64_t realtimeAllocations = 0;

std::size_t allocationSize (void* memory, std::size_t requested) noexcept
{
#if defined (__APPLE__)
    juce::ignoreUnused (requested);
    return memory != nullptr ? malloc_size (memory) : 0;
#elif defined (__linux__)
    juce::ignoreUnused (requested);
    return memory != nullptr ? malloc_usable_size (memory) : 0;
#else
    return memory != nullptr ? requested : 0;
#endif
}

void* allocate (std::size_t size, std::size_t alignment = alignof (std::max_align_t))
{
    void* memory = nullptr;
    if (alignment <= alignof (std::max_align_t))
        memory = std::malloc (size);
    else if (posix_memalign (&memory, alignment, size) != 0)
        memory = nullptr;
    if (memory == nullptr)
        throw std::bad_alloc();
    liveAllocatedBytes.fetch_add (allocationSize (memory, size), std::memory_order_relaxed);
    if (countRealtimeAllocations)
        ++realtimeAllocations;
    return memory;
}

void deallocate (void* memory) noexcept
{
    if (memory == nullptr)
        return;
    liveAllocatedBytes.fetch_sub (allocationSize (memory, 0), std::memory_order_relaxed);
    std::free (memory);
}
}

void* operator new (std::size_t size) { return allocate (size); }
void* operator new[] (std::size_t size) { return allocate (size); }
void* operator new (std::size_t size, std::align_val_t alignment)
{
    return allocate (size, static_cast<std::size_t> (alignment));
}
void* operator new[] (std::size_t size, std::align_val_t alignment)
{
    return allocate (size, static_cast<std::size_t> (alignment));
}
void operator delete (void* memory) noexcept { deallocate (memory); }
void operator delete[] (void* memory) noexcept { deallocate (memory); }
void operator delete (void* memory, std::size_t) noexcept { deallocate (memory); }
void operator delete[] (void* memory, std::size_t) noexcept { deallocate (memory); }
void operator delete (void* memory, std::align_val_t) noexcept { deallocate (memory); }
void operator delete[] (void* memory, std::align_val_t) noexcept { deallocate (memory); }
void operator delete (void* memory, std::size_t, std::align_val_t) noexcept { deallocate (memory); }
void operator delete[] (void* memory, std::size_t, std::align_val_t) noexcept { deallocate (memory); }

namespace
{
using Clock = std::chrono::steady_clock;

class RealtimeAllocationProbe
{
public:
    RealtimeAllocationProbe()
    {
        realtimeAllocations = 0;
        countRealtimeAllocations = true;
    }

    ~RealtimeAllocationProbe() { countRealtimeAllocations = false; }
    std::uint64_t count() const noexcept { return realtimeAllocations; }
};

class HeadlessAudioDevice final : public juce::AudioIODevice
{
public:
    HeadlessAudioDevice() : juce::AudioIODevice ("APW deterministic device", "headless")
    {
        inputs.setRange (0, 2, true);
        outputs.setRange (0, 2, true);
    }

    juce::StringArray getOutputChannelNames() override { return { "out-1", "out-2" }; }
    juce::StringArray getInputChannelNames() override { return { "in-1", "in-2" }; }
    juce::Array<double> getAvailableSampleRates() override { return { 44'100.0, 48'000.0, 96'000.0 }; }
    juce::Array<int> getAvailableBufferSizes() override { return { 64, 127, 512, 4096 }; }
    int getDefaultBufferSize() override { return 512; }

    juce::String open (const juce::BigInteger& requestedInputs,
                       const juce::BigInteger& requestedOutputs,
                       double requestedRate, int requestedBlock) override
    {
        inputs = requestedInputs;
        outputs = requestedOutputs;
        sampleRate = requestedRate;
        blockSize = requestedBlock;
        opened = true;
        return {};
    }

    void close() override { stop(); opened = false; }
    bool isOpen() override { return opened; }

    void start (juce::AudioIODeviceCallback* next) override
    {
        callback = next;
        playing = callback != nullptr;
        if (callback != nullptr)
            callback->audioDeviceAboutToStart (this);
    }

    void stop() override
    {
        if (callback != nullptr)
            callback->audioDeviceStopped();
        callback = nullptr;
        playing = false;
    }

    bool isPlaying() override { return playing; }
    juce::String getLastError() override { return {}; }
    int getCurrentBufferSizeSamples() override { return blockSize; }
    double getCurrentSampleRate() override { return sampleRate; }
    int getCurrentBitDepth() override { return 32; }
    juce::BigInteger getActiveOutputChannels() const override { return outputs; }
    juce::BigInteger getActiveInputChannels() const override { return inputs; }
    int getOutputLatencyInSamples() override { return 0; }
    int getInputLatencyInSamples() override { return 0; }

    void render (int samples, std::uint64_t sequence)
    {
        jassert (callback != nullptr && samples <= blockSize);
        for (int channel = 0; channel < 2; ++channel)
        {
            auto* source = input[channel];
            for (int sample = 0; sample < samples; ++sample)
            {
                const auto phase = static_cast<double> (
                    (sequence * 131 + static_cast<std::uint64_t> (sample * 37 + channel * 11))
                    % 1021) / 1021.0;
                source[sample] = static_cast<float> (phase * 1.6 - 0.8);
                output[channel][sample] = std::numeric_limits<float>::quiet_NaN();
            }
            inputPointers[channel] = source;
            outputPointers[channel] = output[channel];
        }

        juce::AudioIODeviceCallbackContext context;
        RealtimeAllocationProbe allocationProbe;
        callback->audioDeviceIOCallbackWithContext (
            inputPointers, 2, outputPointers, 2, samples, context);
        allocationViolations += allocationProbe.count();

        for (int channel = 0; channel < 2; ++channel)
            if (std::memcmp (input[channel], output[channel],
                             static_cast<std::size_t> (samples) * sizeof (float)) != 0)
                ++transparencyViolations;
    }

    std::uint64_t allocationViolationCount() const noexcept { return allocationViolations; }
    std::uint64_t transparencyViolationCount() const noexcept { return transparencyViolations; }

private:
    double sampleRate = 48'000.0;
    int blockSize = 512;
    bool opened = false;
    bool playing = false;
    juce::BigInteger inputs;
    juce::BigInteger outputs;
    juce::AudioIODeviceCallback* callback = nullptr;
    alignas (64) float input[2][4096] {};
    alignas (64) float output[2][4096] {};
    const float* inputPointers[2] {};
    float* outputPointers[2] {};
    std::uint64_t allocationViolations = 0;
    std::uint64_t transparencyViolations = 0;
};

class LoopbackAckDaemon final
{
public:
    explicit LoopbackAckDaemon (int requestedPort = 0, std::string instance = "headless-daemon")
        : instanceId (std::move (instance))
    {
        if (! socket.bindToPort (requestedPort, "127.0.0.1"))
            return;
        portNumber = socket.getBoundPort();
        worker = std::thread ([this] { run(); });
    }

    ~LoopbackAckDaemon()
    {
        stop.store (true, std::memory_order_relaxed);
        socket.shutdown();
        if (worker.joinable())
            worker.join();
    }

    int port() const noexcept { return portNumber; }
    std::uint64_t receipts() const noexcept { return receiptCount.load (std::memory_order_relaxed); }

private:
    void run()
    {
        char bytes[8192] {};
        while (! stop.load (std::memory_order_relaxed))
        {
            if (socket.waitUntilReady (true, 25) <= 0)
                continue;
            juce::String sender;
            int senderPort = 0;
            const auto size = socket.read (bytes, static_cast<int> (sizeof (bytes) - 1), false,
                                           sender, senderPort);
            if (size <= 0)
                continue;
            bytes[size] = 0;
            const auto event = juce::JSON::parse (juce::String::fromUTF8 (bytes, size));
            const auto* object = event.getDynamicObject();
            if (object == nullptr)
                continue;
            const auto sequence = static_cast<juce::int64> (
                object->getProperty ("event_sequence"));
            auto* acknowledgement = new juce::DynamicObject();
            acknowledgement->setProperty ("message_type", "daemon_receipt_acknowledgement");
            acknowledgement->setProperty ("protocol", "apw-local-udp-ack-v1");
            acknowledgement->setProperty ("plugin_instance_id",
                                           object->getProperty ("plugin_instance_id"));
            acknowledgement->setProperty ("plugin_capture_session_id",
                                           object->getProperty ("plugin_capture_session_id"));
            acknowledgement->setProperty ("daemon_instance_id", juce::String (instanceId));
            acknowledgement->setProperty ("accepted", true);
            acknowledgement->setProperty ("highest_accepted_sequence", sequence);
            acknowledgement->setProperty ("highest_contiguous_sequence", sequence);
            acknowledgement->setProperty ("stream_gaps", 0);
            acknowledgement->setProperty ("stream_rejections", 0);
            acknowledgement->setProperty ("stream_chain_breaks", 0);
            const auto json = juce::JSON::toString (juce::var (acknowledgement), true);
            if (socket.write (sender, senderPort, json.toRawUTF8(),
                              static_cast<int> (json.getNumBytesAsUTF8())) > 0)
                receiptCount.fetch_add (1, std::memory_order_relaxed);
        }
    }

    juce::DatagramSocket socket;
    std::string instanceId;
    int portNumber = -1;
    std::thread worker;
    std::atomic<bool> stop { false };
    std::atomic<std::uint64_t> receiptCount { 0 };
};

int reserveLoopbackPort()
{
    juce::DatagramSocket socket;
    if (! socket.bindToPort (0, "127.0.0.1"))
        return -1;
    return socket.getBoundPort();
}

bool waitUntil (const std::function<bool()>& predicate, int timeoutMilliseconds)
{
    const auto deadline = Clock::now() + std::chrono::milliseconds (timeoutMilliseconds);
    while (Clock::now() < deadline)
    {
        if (predicate())
            return true;
        std::this_thread::sleep_for (std::chrono::milliseconds (5));
    }
    return predicate();
}

struct Options
{
    int soakSeconds = 0;
    std::uint64_t seed = 0x4150'5753'4f41'4b31ULL;
};

bool parseOptions (int argc, char** argv, Options& options)
{
    for (int index = 1; index < argc; ++index)
    {
        const std::string flag (argv[index]);
        if ((flag == "--soak-seconds" || flag == "--seed") && index + 1 >= argc)
            return false;
        if (flag == "--soak-seconds")
            options.soakSeconds = std::max (0, std::atoi (argv[++index]));
        else if (flag == "--seed")
            options.seed = std::strtoull (argv[++index], nullptr, 10);
        else
            return false;
    }
    return true;
}

int failures = 0;

void check (bool condition, const std::string& message)
{
    if (condition)
        return;
    ++failures;
    std::cerr << "FAIL: " << message << '\n';
}

void renderBlocks (HeadlessAudioDevice& device, std::uint64_t& sequence, int count, int size = 512)
{
    for (int block = 0; block < count; ++block)
        device.render (size, sequence++);
}

std::uint64_t renderSoakSegment (HeadlessAudioDevice& device, std::uint64_t& sequence,
                                 std::uint64_t& callbacks, std::chrono::seconds duration)
{
    const auto deadline = Clock::now() + duration;
    std::uint64_t rendered = 0;
    while (Clock::now() < deadline)
    {
        const int sizes[] { 64, 127, 512, 4096, 511, 7 };
        const auto size = sizes[callbacks % (sizeof (sizes) / sizeof (sizes[0]))];
        device.render (size, sequence++);
        ++callbacks;
        ++rendered;
    }
    return rendered;
}

// IMPORTANT: a returned audio callback does not mean its window has been
// accounted for; the observer hashes on its own thread. Sampling live
// allocation while that thread is mid-window measures a transient, and a
// difference of two such samples is not a steady-state claim.
void quiesceObserver (const apw::AudioObserver& observer)
{
    auto previous = observer.getTotalWindowsHashed();
    for (int stableChecks = 0; stableChecks < 5;)
    {
        std::this_thread::sleep_for (std::chrono::milliseconds (20));
        const auto current = observer.getTotalWindowsHashed();
        stableChecks = current == previous ? stableChecks + 1 : 0;
        previous = current;
    }
}

void testLifecycleMatrix()
{
    HeadlessAudioDevice device;
    juce::BigInteger channels;
    channels.setRange (0, 2, true);
    check (device.open (channels, channels, 48'000.0, 512).isEmpty(),
           "headless device did not open");
    juce::AudioProcessorPlayer player;
    device.start (&player);
    std::uint64_t sequence = 0;

    // Daemon before plug-in.
    LoopbackAckDaemon daemonBefore;
    check (daemonBefore.port() > 0, "daemon-before socket did not bind");
    auto processor = std::make_unique<AudioProvenanceCaptureAudioProcessor> (daemonBefore.port());
    player.setProcessor (processor.get());
    renderBlocks (device, sequence, 48);
    const auto daemonBeforeAcked = waitUntil ([&] {
        return processor->getEventEmitter().getAcknowledgementSnapshot()
                   .acknowledgementsProcessed > 0;
    }, 2000);
    check (daemonBeforeAcked, "daemon-before lifecycle produced no acknowledgement (attempts "
               + std::to_string (processor->getEventEmitter().getSendAttempts())
               + ", accepted " + std::to_string (processor->getEventEmitter().getSendAccepted())
               + ", daemon receipts " + std::to_string (daemonBefore.receipts()) + ")");

    // Project save, plug-in deletion, and reload through the host state chunk.
    processor->setSigningArmed (true);
    juce::MemoryBlock projectState;
    processor->getStateInformation (projectState);
    const auto oldInstance = processor->getPluginInstanceId();
    player.setProcessor (nullptr);
    const auto deleteStarted = Clock::now();
    processor.reset();
    const auto deleteMilliseconds = std::chrono::duration_cast<std::chrono::milliseconds> (
        Clock::now() - deleteStarted).count();
    check (deleteMilliseconds < 1500, "plug-in deletion exceeded the 1.5 second bound");

    processor = std::make_unique<AudioProvenanceCaptureAudioProcessor> (daemonBefore.port());
    processor->setStateInformation (projectState.getData(), static_cast<int> (projectState.getSize()));
    check (processor->isSigningArmed(), "saved project state was not restored");
    check (processor->getPluginInstanceId() != oldInstance,
           "project reload reused a plug-in lifecycle identifier");
    player.setProcessor (processor.get());
    renderBlocks (device, sequence, 16);

    // Host bypass and re-enable. AudioProcessorPlayer owns normal callbacks; the bypass callback is
    // the explicit host branch JUCE exposes to DAWs.
    juce::AudioBuffer<float> bypass (2, 512);
    for (int channel = 0; channel < bypass.getNumChannels(); ++channel)
        for (int sample = 0; sample < bypass.getNumSamples(); ++sample)
            bypass.setSample (channel, sample, static_cast<float> (sample - 255) / 512.0f);
    juce::AudioBuffer<float> bypassBefore;
    bypassBefore.makeCopyOf (bypass);
    juce::MidiBuffer midi;
    processor->processBlockBypassed (bypass, midi);
    check (std::memcmp (bypassBefore.getReadPointer (0), bypass.getReadPointer (0),
                        static_cast<std::size_t> (bypass.getNumSamples()) * sizeof (float)) == 0,
           "host bypass changed audio");
    renderBlocks (device, sequence, 16);
    check (processor->getAudioObserver().getBypassedBuffers() == 1,
           "host bypass toggle was not counted exactly");

    player.setProcessor (nullptr);
    processor.reset();

    // Plug-in before daemon, followed by daemon restart on the same endpoint.
    const auto delayedPort = reserveLoopbackPort();
    check (delayedPort > 0, "could not reserve the delayed daemon port");
    processor = std::make_unique<AudioProvenanceCaptureAudioProcessor> (delayedPort);
    player.setProcessor (processor.get());
    renderBlocks (device, sequence, 48);
    check (processor->getEventEmitter().getAcknowledgementSnapshot()
               .acknowledgementsProcessed == 0,
           "plug-in-before-daemon received an impossible acknowledgement");
    {
        LoopbackAckDaemon daemonAfter (delayedPort, "daemon-after");
        check (daemonAfter.port() == delayedPort, "daemon-after socket did not bind reserved port");
        renderBlocks (device, sequence, 48);
        const auto daemonAfterAcked = waitUntil ([&] {
            return processor->getEventEmitter().getAcknowledgementSnapshot()
                       .acknowledgementsProcessed > 0;
        }, 2000);
        check (daemonAfterAcked, "plug-in-before-daemon did not recover when daemon started "
                   "(attempts " + std::to_string (processor->getEventEmitter().getSendAttempts())
                   + ", accepted " + std::to_string (processor->getEventEmitter().getSendAccepted())
                   + ", daemon receipts " + std::to_string (daemonAfter.receipts()) + ")");
    }
    const auto receiptsBeforeRestart = processor->getEventEmitter()
                                           .getAcknowledgementSnapshot()
                                           .acknowledgementsProcessed;
    {
        LoopbackAckDaemon restarted (delayedPort, "daemon-restarted");
        check (restarted.port() == delayedPort, "restarted daemon did not bind prior endpoint");
        renderBlocks (device, sequence, 48);
        check (waitUntil ([&] {
            const auto ack = processor->getEventEmitter().getAcknowledgementSnapshot();
            return ack.acknowledgementsProcessed > receiptsBeforeRestart
                && ack.daemonRestartsObserved > 0;
        }, 2000), "daemon restart was not recovered and surfaced");
    }

    player.setProcessor (nullptr);
    processor.reset();
    device.stop();
    check (device.allocationViolationCount() == 0,
           "AudioProcessorPlayer lifecycle callback allocated on the real-time thread");
    check (device.transparencyViolationCount() == 0,
           "AudioProcessorPlayer lifecycle callback changed pass-through samples");
}

void runSoak (const Options& options)
{
    if (options.soakSeconds <= 0)
        return;

    const auto unusedPort = reserveLoopbackPort();
    HeadlessAudioDevice device;
    juce::BigInteger channels;
    channels.setRange (0, 2, true);
    check (device.open (channels, channels, 48'000.0, 4096).isEmpty(),
           "soak device did not open");
    juce::AudioProcessorPlayer player;
    device.start (&player);
    auto processor = std::make_unique<AudioProvenanceCaptureAudioProcessor> (unusedPort);
    player.setProcessor (processor.get());

    std::uint64_t sequence = options.seed;
    std::uint64_t callbacks = 0;

    // Some allocations reachable from the audio path are one-time lazy
    // initialisation rather than steady state: the observer's hash chain holds
    // nothing until its first window, and JUCE interns an event's JSON property
    // names in a global string pool the first time that variant is emitted, so
    // a rare variant pays for its names whenever it first fires. Measuring
    // across them reports initialisation as growth. A fixed warm-up is not
    // enough to clear them -- how long it takes depends on when the content
    // first crosses each event threshold -- so steady state is observed rather
    // than assumed: absorb whole segments until two consecutive ones retain
    // nothing, and only then open the measured window.
    auto settledBytes = liveAllocatedBytes.load (std::memory_order_relaxed);
    std::int64_t lazyInitBytes = 0;
    int absorbSegments = 0;
    for (int settledSegments = 0;
         settledSegments < 2 && absorbSegments < 30;
         ++absorbSegments)
    {
        renderSoakSegment (device, sequence, callbacks, std::chrono::seconds (1));
        quiesceObserver (processor->getAudioObserver());
        const auto current = liveAllocatedBytes.load (std::memory_order_relaxed);
        const auto delta = static_cast<std::int64_t> (current)
            - static_cast<std::int64_t> (settledBytes);
        lazyInitBytes += delta;
        settledSegments = delta == 0 ? settledSegments + 1 : 0;
        settledBytes = current;
    }
    check (absorbSegments < 30,
           "live allocation never settled across 30 absorb segments, retaining "
               + std::to_string (lazyInitBytes) + " bytes");

    const auto measuredCallbacks = renderSoakSegment (
        device, sequence, callbacks, std::chrono::seconds (options.soakSeconds));
    quiesceObserver (processor->getAudioObserver());
    const auto measuredBytes = liveAllocatedBytes.load (std::memory_order_relaxed);

    // Two-sided. The former one-sided clamp reported zero growth whenever the
    // baseline was the larger sample, so a baseline taken with transients in
    // flight -- which is what an unquiesced one is -- hid real movement.
    const auto growthBytes = static_cast<std::int64_t> (measuredBytes)
        - static_cast<std::int64_t> (settledBytes);
    check (growthBytes == 0, "steady-state live allocation moved by "
                                + std::to_string (growthBytes) + " bytes");

    // Quiescing for the measurement drains the FIFO, so refill it faster than
    // the observer can hash: the shutdown bound below is only worth asserting
    // when there is more ready evidence than the flush is allowed to keep.
    renderBlocks (device, sequence, 64, 4096);

    // Stopping the observer accounts every queued or partial sample and exposes the exact bounded
    // shutdown decision while the object is still inspectable.
    processor->getAudioObserver().stop();
    const auto report = processor->getAudioObserver().getShutdownReport();
    const auto expectedFlush = std::min (
        report.readySamplesBeforeFlush
            / static_cast<std::uint64_t> (processor->getAudioObserver().getWindowSize()),
        report.maximumWindows);
    check (report.windowsFlushed == expectedFlush,
           "shutdown did not flush the exact min(ready windows, four) bound");
    const auto submitted = processor->getAudioObserver().getSamplesSubmitted();
    const auto represented = processor->getAudioObserver().getTotalWindowsHashed()
        * static_cast<std::uint64_t> (processor->getAudioObserver().getWindowSize())
        + processor->getAudioObserver().getFifoSamplesDropped();
    check (submitted == represented,
           "FIFO conservation failed: submitted samples were neither hashed nor counted dropped");

    check (device.allocationViolationCount() == 0,
           "soak observed real-time allocation(s): "
               + std::to_string (device.allocationViolationCount()));
    check (device.transparencyViolationCount() == 0,
           "soak observed pass-through sample changes");

    std::cout << "{\"schema\":\"apw-headless-soak-v1\",\"seed\":" << options.seed
              << ",\"wall_seconds\":" << options.soakSeconds
              << ",\"absorb_segments\":" << absorbSegments
              << ",\"callbacks\":" << measuredCallbacks
              << ",\"lazy_init_bytes\":" << lazyInitBytes
              << ",\"memory_growth_bytes\":" << growthBytes
              << ",\"realtime_allocation_violations\":"
              << device.allocationViolationCount()
              << ",\"transparency_violations\":" << device.transparencyViolationCount()
              << ",\"shutdown_ready_samples\":" << report.readySamplesBeforeFlush
              << ",\"shutdown_windows_flushed\":" << report.windowsFlushed
              << ",\"shutdown_samples_discarded\":" << report.samplesDiscardedAfterFlush
              << ",\"fifo_samples_submitted\":" << submitted
              << ",\"fifo_samples_accounted\":" << represented << "}\n";

    player.setProcessor (nullptr);
    processor.reset();
    device.stop();
}
}

int main (int argc, char** argv)
{
    Options options;
    if (! parseOptions (argc, argv, options))
    {
        std::cerr << "usage: AudioProvenanceHeadlessHostTests [--soak-seconds N] [--seed N]\n";
        return 2;
    }

    juce::ScopedJuceInitialiser_GUI juceInitialiser;
    testLifecycleMatrix();
    runSoak (options);
    if (failures == 0)
    {
        std::cout << "headless DAW lifecycle/soak matrix passed\n";
        return 0;
    }
    std::cerr << failures << " headless lifecycle/soak failure(s)\n";
    return 1;
}
